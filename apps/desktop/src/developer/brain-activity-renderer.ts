import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { activityChangeScale, activityDisplayIntensities, type ActivityDisplayMode, type RateActivity } from "../brain-activity.js";

export interface BrainRenderer {
  readonly setFrame: (index: number, mode: ActivityDisplayMode) => void;
  readonly rotate: (horizontal: number, vertical: number) => void;
  readonly zoom: (factor: number) => void;
  readonly reset: () => void;
  readonly dispose: () => void;
}

export function createBrainRenderer(host: HTMLDivElement, activity: RateActivity, onContextLost: () => void): BrainRenderer {
  const changeScale = activityChangeScale(activity);
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  renderer.setClearColor(0x080f1c);
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(45, 1, 0.01, 100);
  camera.position.set(0, 0, 3.4);
  const controls = new OrbitControls(camera, renderer.domElement);
  controls.enablePan = false;
  controls.minDistance = 1.1;
  controls.maxDistance = 12;
  const geometry = new THREE.BufferGeometry();
  const material = new THREE.ShaderMaterial({
    transparent: true, depthWrite: false, blending: THREE.AdditiveBlending,
    vertexShader: `attribute float intensity; varying float strength; varying float direction;
      void main() { direction = intensity; strength = abs(intensity); gl_PointSize = 3.0 + 9.0 * strength;
      gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }`,
    fragmentShader: `varying float strength; varying float direction;
      void main() { float r = length(gl_PointCoord - vec2(0.5)); if (r > 0.5) discard;
      vec3 color = mix(vec3(0.05, 0.16, 0.27), direction < 0.0 ? vec3(0.2, 0.6, 1.0) : vec3(1.0, 0.58, 0.13), strength);
      gl_FragColor = vec4(color, exp(-12.0 * r * r) * (0.35 + strength * 0.65)); }`,
  });
  const bounds = new THREE.Box3();
  for (const neuron of activity.neurons) bounds.expandByPoint(new THREE.Vector3(...neuron.position));
  const center = bounds.getCenter(new THREE.Vector3());
  const extent = bounds.getSize(new THREE.Vector3());
  const scale = 2 / Math.max(extent.x, extent.y, extent.z, 1);
  const positions = new Float32Array(activity.neurons.length * 3);
  activity.neurons.forEach((neuron, index) => {
    const point = new THREE.Vector3(...neuron.position).sub(center).multiplyScalar(scale);
    positions.set([point.x, -point.y, -point.z], index * 3);
  });
  geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  const intensities = new THREE.BufferAttribute(new Float32Array(activity.neurons.length), 1);
  geometry.setAttribute("intensity", intensities);
  scene.add(new THREE.Points(geometry, material));
  host.append(renderer.domElement);
  let disposed = false;
  const render = (): void => { if (!disposed) renderer.render(scene, camera); };
  const resize = (): void => {
    if (disposed) return;
    const width = Math.max(host.clientWidth, 1);
    const height = Math.max(host.clientHeight, 1);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    renderer.setSize(width, height, false);
    render();
  };
  const observer = new ResizeObserver(resize);
  observer.observe(host);
  controls.addEventListener("change", render);
  const contextLost = (event: Event): void => { event.preventDefault(); onContextLost(); };
  renderer.domElement.addEventListener("webglcontextlost", contextLost);
  resize();
  return {
    setFrame: (index, mode) => {
      const values = activityDisplayIntensities(activity, index, mode, changeScale);
      for (let neuron = 0; neuron < values.length; neuron += 1) intensities.setX(neuron, values[neuron]!);
      intensities.needsUpdate = true;
      render();
    },
    rotate: (horizontal, vertical) => {
      camera.position.applyAxisAngle(new THREE.Vector3(0, 1, 0), horizontal);
      const right = new THREE.Vector3().crossVectors(camera.up, camera.position).normalize();
      camera.position.applyAxisAngle(right, vertical);
      controls.update(); render();
    },
    zoom: (factor) => { camera.position.multiplyScalar(factor).clampLength(controls.minDistance, controls.maxDistance); controls.update(); render(); },
    reset: () => { controls.reset(); render(); },
    dispose: () => {
      disposed = true;
      observer.disconnect();
      controls.removeEventListener("change", render);
      renderer.domElement.removeEventListener("webglcontextlost", contextLost);
      controls.dispose(); geometry.dispose(); material.dispose(); renderer.dispose(); renderer.forceContextLoss();
      renderer.domElement.remove();
    },
  };
}
