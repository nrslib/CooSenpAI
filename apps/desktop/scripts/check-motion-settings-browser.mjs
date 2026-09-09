import { pathToFileURL } from 'node:url';
import { readFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';

const [playwrightPath, baseUrl, outputDirectory, modelPath] = process.argv.slice(2);
if (!playwrightPath || !baseUrl || !outputDirectory || !modelPath) throw new Error('使用法: node scripts/check-motion-settings-browser.mjs <playwright/index.mjs> <Vite URL> <出力先> <モデル.vrm>');
await mkdir(outputDirectory, { recursive: true });
const { chromium } = await import(pathToFileURL(playwrightPath).href);
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1000, height: 850 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/motion-settings-check', route => route.fulfill({ contentType: 'text/html', body: '<html><body><main id="root"></main></body></html>' }));
  await page.route('**/sample.vrm', async route => route.fulfill({ contentType: 'application/octet-stream', body: await readFile(modelPath) }));
  await page.route('**/src/ipc.ts*', route => route.fulfill({ contentType: 'application/javascript', body: 'export const avatarApi = { show: async () => { window.updateAvatar({visible:true}); return {ok:true,value:{visible:true}}; } };' }));
  async function mount() {
    await page.goto(new URL('/motion-settings-check', baseUrl).href);
    await page.evaluate(async () => {
      const refresh = await import('/@react-refresh'); refresh.default.injectIntoGlobalHook(window);
      window.$RefreshReg$ = () => {}; window.$RefreshSig$ = () => type => type;
      window.__vite_plugin_react_preamble_installed__ = true;
      const { React, createRoot, AvatarScene, neutralEmotions } = await import('/scripts/vrm-native-entry.ts');
      const { default: MotionControls } = await import('/src/vrm/MotionControls.tsx');
      await import('/src/styles.css');
      await import('/src/vrm/vrm.css');
      let snapshot = { revision: 0, modelRevision: 0, visible: true, emotions: neutralEmotions(), emotionsEnabled: true, thinking: false, replyId: null };
      const { saveVrmModel } = await import('/src/vrm/model-store.ts');
      await saveVrmModel({ name: 'sample.vrm', data: await (await fetch('/sample.vrm')).arrayBuffer() });
      const root = createRoot(document.getElementById('root'));
      window.updateAvatar = patch => {
        snapshot = { ...snapshot, ...patch };
        root.render(React.createElement(React.StrictMode, null,
          React.createElement('div', { style: { width: 500, height: 800, overflow: 'auto', background: 'var(--bg)', padding: 12, boxSizing: 'border-box' } }, React.createElement(MotionControls)),
          React.createElement('div', { className: 'avatar-overlay', style: { position: 'fixed', right: 0, top: 0, width: 450, height: 800 } }, React.createElement(AvatarScene, { snapshot }))));
      };
      window.updateAvatar({});
    });
    await page.locator('canvas').waitFor();
  }
  await mount();
  await page.getByLabel('喜びのモーション', { exact: true }).selectOption('reply');
  await page.getByRole('status').filter({ hasText: '喜びのモーションを保存しました' }).waitFor();
  await page.getByLabel('持ち込むモーションの利用条件を確認しました').check();
  await page.getByLabel('考え中の VRMA ファイル', { exact: true }).setInputFiles(resolve('public/motions/idle-7.vrma'));
  await page.getByRole('status').filter({ hasText: '考え中のモーションを保存しました' }).waitFor();
  await page.getByRole('group', { name: '考え中', exact: true }).getByRole('button', { name: '試す', exact: true }).click();
  await page.getByRole('status').filter({ hasText: '考え中を試しています' }).waitFor();
  await page.waitForTimeout(1200);
  await page.screenshot({ path: resolve(outputDirectory, 'motion-settings-preview.png') });
  await mount();
  assert.equal(await page.getByLabel('考え中のモーション', { exact: true }).inputValue(), 'file');
  assert.equal(await page.getByLabel('喜びのモーション', { exact: true }).inputValue(), 'reply');
  await page.getByLabel('持ち込むモーションの利用条件を確認しました').check();
  await page.getByLabel('考え中の VRMA ファイル', { exact: true }).setInputFiles({ name: 'broken.vrma', mimeType: 'application/octet-stream', buffer: Buffer.from('not a vrma') });
  await page.getByRole('alert').filter({ hasText: 'モーションを変更できません' }).waitFor();
  assert.equal(await page.getByLabel('考え中のモーション', { exact: true }).inputValue(), 'file');
  await page.getByRole('group', { name: '考え中', exact: true }).getByRole('button', { name: '標準に戻す' }).click();
  await page.getByRole('status').filter({ hasText: '考え中のモーションを保存しました' }).waitFor();
  assert.equal(await page.getByLabel('考え中のモーション', { exact: true }).inputValue(), 'none');
  await mount();
  assert.equal(await page.getByLabel('考え中のモーション', { exact: true }).inputValue(), 'none');
  const failures = await page.evaluate(async () => {
    const { saveMotionSelection, readMotionSettings } = await import('/src/vrm/motion-settings.ts');
    const OriginalChannel = window.BroadcastChannel;
    window.BroadcastChannel = class { postMessage() { throw new Error('notification failed'); } close() {} };
    let notification;
    try { notification = await saveMotionSelection('concern', { kind: 'builtin', id: 'reply' }, new AbortController().signal); }
    finally { window.BroadcastChannel = OriginalChannel; }
    const saved = (await readMotionSettings()).concern;
    const controller = new AbortController();
    const originalPut = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function (...args) {
      const request = originalPut.apply(this, args);
      queueMicrotask(() => controller.abort());
      return request;
    };
    let aborted = false;
    try { await saveMotionSelection('concern', { kind: 'none' }, controller.signal); }
    catch { aborted = true; }
    finally { IDBObjectStore.prototype.put = originalPut; }
    const afterAbort = (await readMotionSettings()).concern;
    await new Promise((resolve, reject) => {
      const request = indexedDB.open('coosenpai-motions', 1);
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result;
        const tx = db.transaction('assignments', 'readwrite');
        tx.objectStore('assignments').put({ kind: 'file', name: 'corrupted.vrma', data: new ArrayBuffer(32) }, 'concern');
        tx.oncomplete = () => { db.close(); resolve(); };
        tx.onabort = () => { db.close(); reject(tx.error); };
      };
    });
    let corruptRejected = false;
    try { await readMotionSettings(); } catch { corruptRejected = true; }
    await saveMotionSelection('concern', { kind: 'none' }, new AbortController().signal);
    return { notification, saved, aborted, afterAbort, corruptRejected };
  });
  assert.deepEqual(failures, { notification: { notified: false }, saved: { kind: 'builtin', id: 'reply' }, aborted: true, afterAbort: { kind: 'builtin', id: 'reply' }, corruptRejected: true });
  assert.deepEqual(errors, []);
  console.log('モーション設定ブラウザ検証: PASS（同梱選択・VRMA持込・プレビュー・再読込復元・不正ファイル拒否・標準復元。IPCはfixture）');
} finally { await browser.close(); }
