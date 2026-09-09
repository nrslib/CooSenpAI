import { vrmError, type VrmErrorKey } from "./errors.js";
import { LoadingManager } from "three";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { VRMExpressionPresetName, VRMHumanBoneName } from "@pixiv/three-vrm";
import { VRMAnimation, VRMAnimationLoaderPlugin } from "@pixiv/three-vrm-animation";

export const MAX_VRMA_BYTES = 8 * 1024 * 1024;
const EXTENSION = "VRMC_vrm_animation";
const MAX_SCALARS = 500_000;
type RecordValue = Record<string, unknown>;

function requireValid(condition: unknown, message: VrmErrorKey): asserts condition {
  if (!condition) throw vrmError(message);
}
function record(value: unknown): RecordValue {
  requireValid(typeof value === "object" && value !== null && !Array.isArray(value), "vrm.errors.motion.invalidObject");
  return value as RecordValue;
}
function array(value: unknown, max: number): unknown[] {
  requireValid(Array.isArray(value) && value.length <= max, "vrm.errors.motion.invalidArray");
  return value;
}
function integer(value: unknown, max: number): number {
  requireValid(typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= max, "vrm.errors.motion.invalidIndex");
  return value;
}
function fields(value: RecordValue, allowed: string[]): void {
  requireValid(Object.keys(value).every(key => allowed.includes(key) || key === "name" || key === "extras"), "vrm.errors.motion.unsupportedFields");
  requireValid(value.name === undefined || typeof value.name === "string", "vrm.errors.motion.invalidName");
}
function vector(value: unknown, size: number, limit: number): number[] {
  const values = array(value, size);
  requireValid(values.length === size && values.every(n => typeof n === "number" && Number.isFinite(n) && Math.abs(n) <= limit), "vrm.errors.motion.invalidVector");
  return values as number[];
}
function quaternion(values: ArrayLike<number>, start: number): void {
  let norm = 0;
  for (let i = 0; i < 4; i++) norm += values[start + i]! ** 2;
  requireValid(Math.abs(norm - 1) <= 0.02, "vrm.errors.motion.invalidQuaternion");
}

function parseGlb(data: ArrayBuffer): { json: RecordValue; binary: DataView } {
  requireValid(data.byteLength >= 28, "vrm.errors.motion.shortGlb");
  const header = new DataView(data);
  requireValid(header.getUint32(0, true) === 0x46546c67 && header.getUint32(4, true) === 2 && header.getUint32(8, true) === data.byteLength, "vrm.errors.motion.invalidHeader");
  const jsonLength = header.getUint32(12, true);
  requireValid(jsonLength > 0 && jsonLength <= 512 * 1024 && jsonLength % 4 === 0 && jsonLength <= data.byteLength - 28 && header.getUint32(16, true) === 0x4e4f534a, "vrm.errors.motion.invalidJsonChunk");
  const binHeader = 20 + jsonLength;
  const binLength = header.getUint32(binHeader, true);
  requireValid(header.getUint32(binHeader + 4, true) === 0x004e4942 && binLength % 4 === 0 && binHeader + 8 + binLength === data.byteLength, "vrm.errors.motion.invalidChunks");
  let json: unknown;
  try { json = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(data, 20, jsonLength))); }
  catch { throw vrmError("vrm.errors.motion.invalidJson"); }
  return { json: record(json), binary: new DataView(data, binHeader + 8, binLength) };
}

function inspectJson(json: RecordValue): void {
  const pending: { value: unknown; depth: number }[] = [{ value: json, depth: 0 }];
  let count = 0;
  while (pending.length) {
    const { value, depth } = pending.pop()!;
    requireValid(++count <= 40_000 && depth <= 32, "vrm.errors.motion.complexJson");
    if (typeof value === "number") requireValid(Number.isFinite(value), "vrm.errors.motion.nonFinite");
    if (typeof value === "string") requireValid(value.length <= 2048, "vrm.errors.motion.longString");
    if (value !== null && typeof value === "object") {
      for (const [key, child] of Object.entries(value)) {
        requireValid(!["uri", "__proto__", "prototype", "constructor"].includes(key), "vrm.errors.motion.externalReference");
        requireValid(key !== "extensions" || value === json, "vrm.errors.motion.nestedExtension");
        pending.push({ value: child, depth: depth + 1 });
      }
    }
  }
}

function inspectNodes(json: RecordValue): Map<number, string> {
  const nodes = array(json.nodes, 128).map(record);
  requireValid(nodes.length > 0, "vrm.errors.motion.missingNodes");
  const parents = new Set<number>();
  const children = nodes.map(node => {
    fields(node, ["children", "translation", "rotation", "scale"]);
    if (node.translation !== undefined) vector(node.translation, 3, 100);
    if (node.rotation !== undefined) quaternion(vector(node.rotation, 4, 1.01), 0);
    if (node.scale !== undefined) requireValid(vector(node.scale, 3, 1.000001).every(n => Math.abs(n - 1) < 0.000001), "vrm.errors.motion.scaledBone");
    return node.children === undefined ? [] : array(node.children, 128).map(child => {
      const index = integer(child, nodes.length - 1);
      requireValid(!parents.has(index), "vrm.errors.motion.multipleParents");
      parents.add(index);
      return index;
    });
  });
  const scenes = array(json.scenes, 1);
  requireValid(scenes.length === 1 && (json.scene === undefined || json.scene === 0), "vrm.errors.motion.multipleScenes");
  const scene = record(scenes[0]); fields(scene, ["nodes"]);
  const roots = array(scene.nodes, 128).map(n => integer(n, nodes.length - 1));
  requireValid(roots.every(n => !parents.has(n)), "vrm.errors.motion.invalidRoot");
  const seen = new Set<number>();
  const pending = roots.map(node => ({ node, depth: 0 }));
  while (pending.length) {
    const { node, depth } = pending.pop()!;
    requireValid(!seen.has(node) && depth <= 32, "vrm.errors.motion.nodeCycle");
    seen.add(node);
    for (const child of children[node]!) pending.push({ node: child, depth: depth + 1 });
  }
  requireValid(seen.size === nodes.length, "vrm.errors.motion.orphanNode");
  const extension = record(record(json.extensions)[EXTENSION]);
  fields(extension, ["specVersion", "humanoid", "expressions", "lookAt"]);
  requireValid(extension.specVersion === "1.0", "vrm.errors.motion.version");
  const humanoid = record(extension.humanoid); fields(humanoid, ["humanBones"]);
  const bones = record(humanoid.humanBones);
  const validBones = new Set<string>(Object.values(VRMHumanBoneName));
  requireValid(Object.hasOwn(bones, "hips"), "vrm.errors.motion.missingHips");
  const targets = new Map<number, string>();
  const add = (definition: unknown, kind: string): void => {
    const item = record(definition); fields(item, ["node"]);
    const index = integer(item.node, nodes.length - 1);
    requireValid(!targets.has(index), "vrm.errors.motion.duplicateAssignment");
    targets.set(index, kind);
  };
  for (const [name, definition] of Object.entries(bones)) {
    requireValid(validBones.has(name), "vrm.errors.motion.unknownBone");
    add(definition, name);
  }
  if (extension.expressions !== undefined) {
    const expressions = record(extension.expressions); fields(expressions, ["preset", "custom"]);
    const presets = new Set<string>(Object.values(VRMExpressionPresetName));
    for (const group of ["preset", "custom"]) {
      if (expressions[group] === undefined) continue;
      const definitions = Object.entries(record(expressions[group]));
      requireValid(definitions.length <= 32, "vrm.errors.motion.tooManyExpressions");
      for (const [name, definition] of definitions) {
        requireValid(name.length > 0 && name.length <= 64 && !/[.\[\]\/\\:]/.test(name), "vrm.errors.motion.expressionName");
        requireValid(group === "preset" ? presets.has(name) && !["lookUp", "lookDown", "lookLeft", "lookRight"].includes(name) : !presets.has(name), "vrm.errors.motion.expressionGroup");
        add(definition, "expression");
      }
    }
  }
  if (extension.lookAt !== undefined) {
    const lookAt = record(extension.lookAt); fields(lookAt, ["node", "offsetFromHeadBone"]);
    if (lookAt.offsetFromHeadBone !== undefined) vector(lookAt.offsetFromHeadBone, 3, 10);
    add({ node: lookAt.node }, "lookAt");
  }
  return targets;
}

function inspectAnimation(json: RecordValue, binary: DataView, targets: Map<number, string>, signal: AbortSignal): void {
  const buffers = array(json.buffers, 1);
  requireValid(buffers.length === 1, "vrm.errors.motion.missingBuffer");
  const buffer = record(buffers[0]); fields(buffer, ["byteLength"]);
  const bytes = integer(buffer.byteLength, binary.byteLength);
  requireValid(binary.byteLength - bytes <= 3, "vrm.errors.motion.bufferLength");
  let viewBytes = 0;
  const views = array(json.bufferViews, 256).map(value => {
    const view = record(value); fields(view, ["buffer", "byteOffset", "byteLength"]);
    requireValid(view.buffer === 0, "vrm.errors.motion.bufferReference");
    const offset = view.byteOffset === undefined ? 0 : integer(view.byteOffset, bytes);
    const length = integer(view.byteLength, bytes - offset);
    requireValid(length > 0 && offset % 4 === 0 && (viewBytes += length) <= MAX_VRMA_BYTES, "vrm.errors.motion.bufferBudget");
    return { offset, length };
  });
  let scalars = 0;
  const accessors = array(json.accessors, 256).map(value => {
    const accessor = record(value); fields(accessor, ["bufferView", "byteOffset", "componentType", "count", "type", "min", "max", "normalized"]);
    requireValid(accessor.componentType === 5126 && (accessor.normalized === undefined || accessor.normalized === false), "vrm.errors.motion.accessorFormat");
    const width = accessor.type === "SCALAR" ? 1 : accessor.type === "VEC3" ? 3 : accessor.type === "VEC4" ? 4 : 0;
    requireValid(width > 0, "vrm.errors.motion.accessorType");
    const count = integer(accessor.count, 7201);
    requireValid(count > 0 && (scalars += count * width) <= MAX_SCALARS, "vrm.errors.motion.keyframeBudget");
    const view = views[integer(accessor.bufferView, views.length - 1)]!;
    const offset = accessor.byteOffset === undefined ? 0 : integer(accessor.byteOffset, view.length);
    requireValid(offset % 4 === 0 && count * width * 4 <= view.length - offset, "vrm.errors.motion.accessorBounds");
    for (const key of ["min", "max"]) if (accessor[key] !== undefined) vector(accessor[key], width, 1000);
    return { width, count, offset: view.offset + offset };
  });
  const animations = array(json.animations, 1);
  requireValid(animations.length === 1, "vrm.errors.motion.animationCount");
  const animation = record(animations[0]); fields(animation, ["channels", "samplers"]);
  const samplers = array(animation.samplers, 128).map(value => {
    const sampler = record(value); fields(sampler, ["input", "output", "interpolation"]);
    requireValid(sampler.interpolation === undefined || sampler.interpolation === "LINEAR" || sampler.interpolation === "STEP", "vrm.errors.motion.interpolation");
    return { input: accessors[integer(sampler.input, accessors.length - 1)]!, output: accessors[integer(sampler.output, accessors.length - 1)]!, interpolation: sampler.interpolation };
  });
  const channels = array(animation.channels, 128);
  requireValid(channels.length > 0, "vrm.errors.motion.missingChannels");
  const unique = new Set<string>();
  let trackScalars = 0;
  let duration = 0;
  for (const value of channels) {
    signal.throwIfAborted();
    const channel = record(value); fields(channel, ["sampler", "target"]);
    const target = record(channel.target); fields(target, ["node", "path"]);
    const kind = targets.get(integer(target.node, 127));
    const path = target.path;
    requireValid(kind !== undefined && kind !== "leftEye" && kind !== "rightEye", "vrm.errors.motion.channelTarget");
    requireValid(kind === "expression" ? path === "translation" : path === "rotation" || (kind === "hips" && path === "translation"), "vrm.errors.motion.boneTransform");
    const key = `${target.node}:${path}`;
    requireValid(!unique.has(key), "vrm.errors.motion.duplicateChannel"); unique.add(key);
    const { input, output, interpolation } = samplers[integer(channel.sampler, samplers.length - 1)]!;
    // The upstream expression conversion does not retain STEP interpolation.
    requireValid(kind !== "expression" || interpolation !== "STEP", "vrm.errors.motion.expressionInterpolation");
    const width = path === "rotation" ? 4 : 3;
    requireValid(input.width === 1 && output.width === width && input.count === output.count, "vrm.errors.motion.accessorMismatch");
    requireValid((trackScalars += input.count * (width + 1)) <= MAX_SCALARS, "vrm.errors.motion.trackBudget");
    let previous = -1;
    for (let i = 0; i < input.count; i++) {
      const time = binary.getFloat32(input.offset + i * 4, true);
      requireValid(Number.isFinite(time) && time >= 0 && time <= 120 && time > previous, "vrm.errors.motion.invalidTime");
      previous = time; duration = Math.max(duration, time);
      const values: number[] = [];
      for (let j = 0; j < width; j++) {
        const n = binary.getFloat32(output.offset + (i * width + j) * 4, true);
        requireValid(Number.isFinite(n) && Math.abs(n) <= 100, "vrm.errors.motion.keyframeValue"); values.push(n);
      }
      if (path === "rotation") quaternion(values, 0);
      if (kind === "expression") requireValid(values[0]! >= 0 && values[0]! <= 1, "vrm.errors.motion.expressionValue");
    }
  }
  requireValid(duration > 0, "vrm.errors.motion.zeroDuration");
}

export async function loadMotionFile(data: ArrayBuffer, signal: AbortSignal): Promise<VRMAnimation> {
  signal.throwIfAborted();
  requireValid(data instanceof ArrayBuffer && data.byteLength <= MAX_VRMA_BYTES, "vrm.errors.motion.tooLarge");
  // Parsing is asynchronous: retain an immutable input snapshot across its awaits.
  const copy = data.slice(0);
  const { json, binary } = parseGlb(copy);
  inspectJson(json);
  fields(json, ["asset", "scene", "scenes", "nodes", "animations", "buffers", "bufferViews", "accessors", "extensions", "extensionsUsed", "extensionsRequired"]);
  const asset = record(json.asset); fields(asset, ["version", "minVersion", "generator", "copyright"]);
  requireValid(asset.version === "2.0" && (asset.minVersion === undefined || asset.minVersion === "2.0"), "vrm.errors.motion.gltfVersion");
  requireValid(Object.keys(record(json.extensions)).length === 1 && Object.hasOwn(record(json.extensions), EXTENSION), "vrm.errors.motion.extension");
  requireValid(array(json.extensionsUsed, 1)[0] === EXTENSION, "vrm.errors.motion.missingExtension");
  if (json.extensionsRequired !== undefined) requireValid(array(json.extensionsRequired, 1).every(value => value === EXTENSION), "vrm.errors.motion.requiredExtension");
  const targets = inspectNodes(json);
  inspectAnimation(json, binary, targets, signal);
  signal.throwIfAborted();
  const manager = new LoadingManager();
  manager.setURLModifier(() => { throw vrmError("vrm.errors.motion.externalNetwork"); });
  const loader = new GLTFLoader(manager);
  loader.register(parser => new VRMAnimationLoaderPlugin(parser));
  let gltf;
  try { gltf = await loader.parseAsync(copy, ""); }
  catch { signal.throwIfAborted(); throw vrmError("vrm.errors.motion.parseFailed"); }
  signal.throwIfAborted();
  const animations: unknown = gltf.userData.vrmAnimations;
  requireValid(Array.isArray(animations) && animations.length === 1 && animations[0] instanceof VRMAnimation, "vrm.errors.motion.generationFailed");
  const motion = animations[0];
  requireValid(Number.isFinite(motion.duration) && motion.duration > 0 && motion.duration <= 120 && motion.restHipsPosition.toArray().every(Number.isFinite) && motion.restHipsPosition.y >= 0.001 && motion.restHipsPosition.y <= 100, "vrm.errors.motion.invalidRestPose");
  return motion;
}
