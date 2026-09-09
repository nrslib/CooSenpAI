import { readFile, mkdir } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { basename, join } from 'node:path';
import assert from 'node:assert/strict';

const [playwrightPath, baseUrl, output, ...models] = process.argv.slice(2);
if (!models.length) throw new Error('使用法: playwright baseUrl output model...');
await mkdir(output, { recursive: true });
const { chromium } = await import(pathToFileURL(playwrightPath));
const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({ viewport: { width: 320, height: 460 }, recordVideo: { dir: output } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/motion-check', route => route.fulfill({ contentType: 'text/html', body: '<html><body style="margin:0;background:#292929"><div id="stage" style="width:320px;height:460px"></div></body></html>' }));
  const pose = () => page.evaluate(() => Object.fromEntries(['hips', 'leftUpperLeg', 'leftLowerLeg', 'leftLowerArm', 'leftHand', 'head'].map(name => {
    const bone = window.model.humanoid.getNormalizedBoneNode(name);
    return [name, [...bone.position.toArray(), ...bone.quaternion.toArray()]];
  })));
  for (const modelPath of models) {
    await page.goto(new URL('/motion-check', baseUrl).href);
    await page.evaluate(async data => {
      const { loadVrmModel, mountVrmScene } = await import('/src/vrm/runtime.ts');
      const { loadCooMotions } = await import('/src/vrm/bundled-motions.ts');
      const { neutralEmotions } = await import('/src/vrm/emotions.ts');
      window.model = await loadVrmModel({ name: 'test.vrm', data: new Uint8Array(data).buffer }, new AbortController().signal, 'medium');
      const motions = await loadCooMotions(new AbortController().signal);
      window.handle = mountVrmScene(document.getElementById('stage'), window.model, neutralEmotions(), false, cause => { throw cause; }, motions);
    }, Array.from(await readFile(modelPath)));
    const initial = await pose();
    await page.waitForTimeout(4000);
    const later = await pose();
    for (const bone of ['hips', 'leftUpperLeg', 'leftLowerLeg', 'leftLowerArm']) assert.notDeepEqual(initial[bone], later[bone], `${basename(modelPath)} ${bone} idle`);
    await page.locator('canvas').screenshot({ path: join(output, `${basename(modelPath)}-idle.png`) });
    await page.evaluate(() => window.handle.reply());
    await page.waitForTimeout(4000);
    const reply = await pose();
    assert.notDeepEqual(reply.leftLowerArm, later.leftLowerArm);
    await page.locator('canvas').screenshot({ path: join(output, `${basename(modelPath)}-reply.png`) });
    await page.waitForTimeout(12000);
    await page.locator('canvas').screenshot({ path: join(output, `${basename(modelPath)}-returned.png`) });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const staticPose = await pose();
    await page.waitForTimeout(200);
    assert.deepEqual(await pose(), staticPose);
    await page.evaluate(() => window.handle.reply());
    await page.waitForTimeout(200);
    assert.deepEqual(await pose(), staticPose);
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.evaluate(async () => {
      window.handle.dispose();
      (await import('/src/vrm/runtime.ts')).disposeVrmModel(window.model);
    });
    assert.equal(await page.locator('canvas').count(), 0);
    console.log(`PASS ${basename(modelPath)}: 全身待機、返答、復帰、ループ越え、reduced-motion、解放`);
  }
  assert.deepEqual(errors, []);
  await context.close();
} finally { await browser.close(); }
