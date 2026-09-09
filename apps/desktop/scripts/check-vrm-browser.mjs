import { pathToFileURL } from 'node:url';
import { basename, join } from 'node:path';
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

const [playwrightPath, baseUrl, ...modelPaths] = process.argv.slice(2);
if (!playwrightPath || !baseUrl) throw new Error('使用法: node scripts/check-vrm-browser.mjs <playwright/index.mjs の絶対パス> <開発サーバーURL> [実モデル...]');
const { chromium } = await import(pathToFileURL(playwrightPath).href);
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 320, height: 460 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/vrm-check', route => route.fulfill({
    contentType: 'text/html',
    headers: { 'Content-Security-Policy': "default-src 'self'; connect-src 'self'; img-src 'self' blob:; style-src 'self' 'unsafe-inline'; script-src 'self'" },
    body: '<html><body><main id="root" class="avatar-overlay"></main></body></html>',
  }));
  async function mount() {
    await page.goto(new URL('/vrm-check', baseUrl).href);
    await page.evaluate(async () => {
      const refresh = await import('/@react-refresh');
      refresh.default.injectIntoGlobalHook(window);
      window.$RefreshReg$ = () => {};
      window.$RefreshSig$ = () => type => type;
      window.__vite_plugin_react_preamble_installed__ = true;
      const { React, createRoot, AvatarScene, neutralEmotions } = await import('/scripts/vrm-native-entry.ts');
      const root = createRoot(document.getElementById('root'));
      let snapshot = { revision: 0, modelRevision: 0, visible: true, emotions: neutralEmotions(), emotionsEnabled: true, thinking: false, replyId: null };
      window.updateAvatar = patch => {
        snapshot = { ...snapshot, ...patch, revision: snapshot.revision + 1 };
        root.render(React.createElement(React.StrictMode, null, React.createElement(AvatarScene, { snapshot })));
      };
      window.reloadAvatar = () => window.updateAvatar({ modelRevision: snapshot.modelRevision + 1 });
      window.updateAvatar({});
    });
  }
  async function save(name, data) {
    await page.evaluate(async ({ name, data }) => {
      const { saveVrmModel, validateVrmFile } = await import('/scripts/vrm-native-entry.ts');
      const model = { name, data: new Uint8Array(data).buffer };
      validateVrmFile(model);
      window.previousCanvas = document.querySelector('canvas');
      await saveVrmModel(model);
      window.reloadAvatar();
    }, { name, data });
    await page.waitForFunction(() => document.querySelector('canvas') && document.querySelector('canvas') !== window.previousCanvas);
    assert.deepEqual(await page.getByRole('alert').allTextContents(), []);
    assert.ok(await page.locator('canvas').evaluate(canvas => canvas.width > 0 && canvas.height > 0));
  }
  await mount();
  const restoredQuality = await page.evaluate(async () => {
    const { readVrmImageQuality, saveVrmImageQuality } = await import('/src/vrm/model-store.ts');
    await saveVrmImageQuality('high');
    return await readVrmImageQuality();
  });
  assert.equal(restoredQuality, 'high');
  for (const version of ['0', '1']) {
    const data = await page.evaluate(async v => Array.from(new Uint8Array((await import('/src/vrm/test-model.ts')).createTestVrm(v))), version);
    await save(`test-${version}.vrm`, data);
  }
  for (const mime of ['image/png', 'image/jpeg']) {
    const data = await page.evaluate(async mime => {
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 2;
      canvas.getContext('2d').fillRect(0, 0, 2, 2);
      const blob = await new Promise(resolve => canvas.toBlob(resolve, mime));
      const { createTexturedTestVrm } = await import('/src/vrm/test-model.ts');
      return Array.from(new Uint8Array(createTexturedTestVrm(new Uint8Array(await blob.arrayBuffer()), mime)));
    }, mime);
    await save('textured.vrm', data);
  }
  await page.evaluate(() => { window.stableCanvas = document.querySelector('canvas'); window.updateAvatar({ emotions: { joy: 70, embarrassment: 60, concern: 20, surprise: 40, curiosity: 80, frustration: 10 } }); });
  await page.waitForTimeout(100);
  assert.ok(await page.evaluate(() => document.querySelector('canvas') === window.stableCanvas));
  await page.evaluate(async () => window.updateAvatar({ emotions: (await import('/src/vrm/emotions.ts')).neutralEmotions() }));
  await page.waitForTimeout(100);
  assert.ok(await page.evaluate(() => document.querySelector('canvas') === window.stableCanvas));
  await page.evaluate(() => window.updateAvatar({ visible: false }));
  await page.waitForFunction(() => !document.querySelector('canvas'));
  await page.evaluate(() => window.updateAvatar({ visible: true }));
  await page.locator('canvas').waitFor();
  assert.ok(await page.evaluate(() => document.querySelector('canvas') !== window.stableCanvas));
  await mount();
  await page.locator('canvas').waitFor();
  const restored = await page.evaluate(async () => (await (await import('/src/vrm/model-store.ts')).readVrmModel()).name);
  assert.equal(restored, 'textured.vrm');
  // Component restore failures are surfaced, not silently swallowed by GLTFLoader.
  await page.evaluate(() => {
    window.originalBitmap = window.createImageBitmap;
    window.createImageBitmap = async () => { throw new Error('texture decode failure'); };
    window.reloadAvatar();
  });
  await page.getByRole('alert').filter({ hasText: 'texture decode failure' }).waitFor();
  await page.evaluate(() => { window.createImageBitmap = window.originalBitmap; });
  await page.getByRole('button', { name: '再読み込み', exact: true }).click();
  await page.locator('canvas').waitFor();
  await page.evaluate(async () => { await (await import('/src/vrm/model-store.ts')).saveVrmModel(undefined); window.reloadAvatar(); });
  await page.getByRole('status').filter({ hasText: 'VRM未設定' }).waitFor();
  assert.equal(await page.locator('canvas').count(), 0);

  const animation = await page.evaluate(async () => {
    const { createTestVrm } = await import('/src/vrm/test-model.ts');
    const { loadVrmModel, mountVrmScene, disposeVrmModel } = await import('/src/vrm/runtime.ts');
    const { neutralEmotions } = await import('/src/vrm/emotions.ts');
    const model = await loadVrmModel({ name: 'motion.vrm', data: createTestVrm('1') }, new AbortController().signal, 'medium');
    const host = document.createElement('div');
    host.style.cssText = 'width:300px;height:200px';
    document.body.append(host);
    const ownFocus = Object.getOwnPropertyDescriptor(document, 'hasFocus');
    Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => false });
    const scene = mountVrmScene(host, model, neutralEmotions(), true);
    try {
      const head = model.humanoid.getNormalizedBoneNode('head');
      await new Promise(resolve => setTimeout(resolve, 150));
      const before = head.rotation.z;
      window.dispatchEvent(new Event('blur'));
      await new Promise(resolve => setTimeout(resolve, 150));
      return { before, after: head.rotation.z };
    } finally {
      scene.dispose(); disposeVrmModel(model); host.remove();
      if (ownFocus) Object.defineProperty(document, 'hasFocus', ownFocus);
      else delete document.hasFocus;
    }
  });
  assert.notEqual(animation.before, animation.after);
  // Native harness performs the real focus transition; this is a synthetic policy regression only.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const still = await page.evaluate(async () => {
    const { createTestVrm } = await import('/src/vrm/test-model.ts');
    const { loadVrmModel, mountVrmScene, disposeVrmModel } = await import('/src/vrm/runtime.ts');
    const { neutralEmotions } = await import('/src/vrm/emotions.ts');
    const model = await loadVrmModel({ name: 'still.vrm', data: createTestVrm('1') }, new AbortController().signal, 'medium');
    const host = document.createElement('div'); document.body.append(host);
    const scene = mountVrmScene(host, model, neutralEmotions(), false);
    try {
      const head = model.humanoid.getNormalizedBoneNode('head');
      scene.update({ ...neutralEmotions(), curiosity: 100, embarrassment: 100 }, false);
      const changed = head.rotation.toArray().slice(0, 3);
      await new Promise(resolve => setTimeout(resolve, 100));
      const paused = head.rotation.toArray().slice(0, 3);
      scene.update(neutralEmotions(), false);
      return { changed, paused, neutral: head.rotation.toArray().slice(0, 3) };
    } finally { scene.dispose(); disposeVrmModel(model); host.remove(); }
  });
  assert.deepEqual(still.changed, still.paused);
  assert.notDeepEqual(still.changed, still.neutral);
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  const failures = await page.evaluate(async () => {
    const { createTestVrm } = await import('/src/vrm/test-model.ts');
    const { loadVrmModel, mountVrmScene, disposeVrmModel } = await import('/src/vrm/runtime.ts');
    const { neutralEmotions } = await import('/src/vrm/emotions.ts');
    const results = [];
    for (const type of ['update', 'context']) {
      const model = await loadVrmModel({ name: 'error.vrm', data: createTestVrm('1') }, new AbortController().signal, 'medium');
      const host = document.createElement('div'); document.body.append(host);
      let message;
      const scene = mountVrmScene(host, model, neutralEmotions(), true, cause => { message = String(cause); });
      try {
        if (type === 'update') model.update = () => { throw new Error('animation failure'); };
        else host.querySelector('canvas').dispatchEvent(new Event('webglcontextlost'));
        await new Promise(resolve => setTimeout(resolve, 150));
        results.push({ message, canvases: host.querySelectorAll('canvas').length });
      } finally { scene.dispose(); disposeVrmModel(model); host.remove(); }
    }
    return results;
  });
  assert.match(failures[0].message, /animation failure/);
  assert.match(failures[1].message, /コンテキスト/);
  assert.ok(failures.every(result => result.canvases === 0));
  const morphCleanup = await page.evaluate(async () => {
    const { createTestVrm, createTestGlb } = await import('/src/vrm/test-model.ts');
    const { validateVrmFile } = await import('/src/vrm/model-file.ts');
    const { loadVrmModel, verifyVrmRender, mountVrmScene, disposeVrmModel } = await import('/src/vrm/runtime.ts');
    const { neutralEmotions } = await import('/src/vrm/emotions.ts');
    const { json, binary } = validateVrmFile({ name: 'morph.vrm', data: createTestVrm('1') });
    json.meshes[0].primitives[0].targets = [{ POSITION: 0 }];
    const model = await loadVrmModel({ name: 'morph.vrm', data: createTestGlb(json, binary) }, new AbortController().signal, 'medium');
    let geometry;
    model.scene.traverse(object => { if (object.isMesh) geometry = object.geometry; });
    const countListeners = () => geometry._listeners?.dispose?.length ?? 0;
    const baseline = countListeners();
    const host = document.createElement('div'); document.body.append(host);
    try {
      verifyVrmRender(model);
      const afterVerify = countListeners();
      const counts = [];
      for (let i = 0; i < 3; i++) {
        mountVrmScene(host, model, neutralEmotions(), false).dispose();
        counts.push(countListeners());
      }
      return { baseline, afterVerify, counts };
    } finally { disposeVrmModel(model); host.remove(); }
  });
  assert.equal(morphCleanup.afterVerify, morphCleanup.baseline);
  assert.ok(morphCleanup.counts.every(count => count === morphCleanup.baseline));
  const textureSharing = await page.evaluate(async () => {
    const { Texture, Scene, Mesh, PlaneGeometry, MeshBasicMaterial, PerspectiveCamera, WebGLRenderer, SRGBColorSpace, NearestFilter } = await import('/node_modules/.vite/deps/three.js');
    const { inspectVrmTextures } = await import('/src/vrm/texture-budget.ts');
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 2;
    const bitmap = await createImageBitmap(canvas);
    const base = new Texture(bitmap); base.needsUpdate = true;
    const uv = base.clone(); uv.offset.x = 0.2;
    const color = base.clone(); color.colorSpace = SRGBColorSpace;
    const sampler = base.clone(); sampler.magFilter = NearestFilter;
    const textures = [base, uv, color, sampler];
    const scene = new Scene();
    const geometry = new PlaneGeometry(1, 1);
    const materials = textures.map(map => new MeshBasicMaterial({ map }));
    for (const material of materials) scene.add(new Mesh(geometry, material));
    const renderer = new WebGLRenderer();
    const camera = new PerspectiveCamera(); camera.position.z = 2;
    try {
      const estimate = inspectVrmTextures(scene, 'medium');
      renderer.render(scene, camera);
      return { estimate: estimate.allocations, actual: renderer.info.memory.textures };
    } finally {
      geometry.dispose();
      for (const material of materials) material.dispose();
      for (const texture of textures) texture.dispose();
      renderer.dispose(); renderer.forceContextLoss(); bitmap.close();
    }
  });
  assert.deepEqual(textureSharing, { estimate: 3, actual: 3 });
  for (const modelPath of modelPaths) {
    const name = basename(modelPath);
    await save(name, Array.from(await readFile(modelPath)));
    if (process.env.VRM_SCREENSHOT_DIR) await page.locator('.avatar-stage').screenshot({ path: join(process.env.VRM_SCREENSHOT_DIR, `${name}.png`) });
    await mount();
    await page.locator('canvas').waitFor();
    assert.deepEqual(await page.getByRole('alert').allTextContents(), [], `${name} 復元`);
    console.log(`PASS: 実モデル ${name} の直接保存・描画・復元`);
  }
  assert.deepEqual(errors, []);
  console.log('PASS: AvatarScene StrictMode, VRM 0/1・PNG/JPEG, modelRevision差し替え, 感情samecanvas, visiblefalse解放, 保存復元・解除, 画像失敗・再試行, 非focus動作方針, reduced-motion静止更新。Controls/IPC/実OSフォーカスは範囲外。');
} finally { await browser.close(); }
