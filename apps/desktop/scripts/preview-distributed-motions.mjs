import { readFile, mkdir } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { basename, join } from 'node:path';

const [playwrightPath, baseUrl, modelPath, output, ...motionPaths] = process.argv.slice(2);
if (!motionPaths.length) throw new Error('playwright baseUrl model output motion... が必要です');
const { chromium } = await import(pathToFileURL(playwrightPath));
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 400, height: 600 } });
  await page.goto(baseUrl);
  await page.evaluate(async data => {
    const T = await import('/node_modules/three/build/three.module.js');
    const { loadVrmModel } = await import('/src/vrm/runtime.ts');
    const vrm = await loadVrmModel({ name: 'preview.vrm', data: new Uint8Array(data).buffer }, new AbortController().signal, 'medium');
    document.body.replaceChildren();
    const renderer = new T.WebGLRenderer({ antialias: true });
    renderer.setSize(400, 600); renderer.setClearColor(0x292929);
    document.body.append(renderer.domElement);
    const scene = new T.Scene(); scene.add(vrm.scene, new T.HemisphereLight(0xffffff, 0x888899, 2));
    const light = new T.DirectionalLight(0xffffff, 2); light.position.set(1, 2, 3); scene.add(light);
    const box = new T.Box3().setFromObject(vrm.scene);
    const center = box.getCenter(new T.Vector3());
    const camera = new T.PerspectiveCamera(30, 2 / 3, 0.01, 1000);
    camera.position.set(center.x, center.y, center.z + box.getSize(new T.Vector3()).y * 2.4); camera.lookAt(center);
    window.preview = { T, vrm, renderer, scene, camera };
  }, Array.from(await readFile(modelPath)));
  for (const motion of motionPaths) {
    const duration = await page.evaluate(async data => {
      const { GLTFLoader } = await import('/node_modules/three/examples/jsm/loaders/GLTFLoader.js');
      const { VRMAnimationLoaderPlugin, createVRMAnimationHumanoidTracks } = await import('/node_modules/@pixiv/three-vrm-animation/lib/three-vrm-animation.module.js');
      const p = window.preview;
      p.mixer?.stopAllAction();
      const loader = new GLTFLoader(); loader.register(parser => new VRMAnimationLoaderPlugin(parser));
      const gltf = await loader.parseAsync(new Uint8Array(data).buffer, '');
      const animation = gltf.userData.vrmAnimations[0];
      const tracks = createVRMAnimationHumanoidTracks(animation, p.vrm.humanoid, p.vrm.meta.metaVersion);
      const clip = new p.T.AnimationClip('preview', animation.duration, [...tracks.rotation.values(), ...tracks.translation.values()]);
      p.mixer = new p.T.AnimationMixer(p.vrm.scene); p.mixer.clipAction(clip).play();
      return animation.duration;
    }, Array.from(await readFile(motion)));
    for (const fraction of [0, 0.25, 0.5, 0.75]) {
      await page.evaluate(time => {
        const p = window.preview; p.mixer.setTime(time); p.vrm.update(0);
        p.renderer.render(p.scene, p.camera);
      }, duration * fraction);
      await page.locator('canvas').screenshot({ path: join(output, `${basename(motion)}-${fraction}.png`) });
    }
    console.log(`${basename(motion)}: ${duration}s`);
  }
} finally { await browser.close(); }
