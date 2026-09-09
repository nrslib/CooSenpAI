// 未信頼の JSON をローダーに渡す前に、参照と展開量を制限する。
import { vrmError } from "./errors.js";

export function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw vrmError("vrm.errors.invalidObject");
  return value as Record<string, unknown>;
}

export function list(value: unknown, max = 8192): unknown[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > max) throw vrmError("vrm.errors.tooManyElements");
  return value;
}

export function integer(value: unknown, max: number): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > max) throw vrmError("vrm.errors.invalidValue");
  return value;
}

function ref<T>(items: T[], index: unknown): T { return items[integer(index, items.length - 1)]!; }
function table(value: unknown, max: number): Record<string, unknown>[] { return list(value, max).map(object); }
function vector(value: unknown, length: number): void {
  const values = list(value, length);
  if (values.length !== length || values.some((item) => typeof item !== "number" || !Number.isFinite(item))) throw vrmError("vrm.errors.invalidVector");
}
const MEMORY_LIMIT = 128 * 1024 * 1024;

export function validateModelResources(json: Record<string, unknown>, binaryLength: number): void {
  const buffers = table(json.buffers, 1);
  for (const buffer of buffers) integer(buffer.byteLength, binaryLength);
  const views = table(json.bufferViews, 4096);
  let copiedBytes = 0;
  for (const view of views) {
    const buffer = ref(buffers, view.buffer);
    const offset = integer(view.byteOffset ?? 0, binaryLength);
    const length = integer(view.byteLength, Number(buffer.byteLength) - offset);
    copiedBytes += length;
    if (view.byteStride !== undefined) integer(view.byteStride, 252);
  }
  integer(copiedBytes, MEMORY_LIMIT);
  const accessors = table(json.accessors, 8192);
  const sizes: Record<string, number> = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT2: 4, MAT3: 9, MAT4: 16 };
  const components: Record<string, number> = { 5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4 };
  const bytes = accessors.map((accessor) => {
    const count = integer(accessor.count, 1_000_000);
    const size = sizes[String(accessor.type)];
    const component = components[String(accessor.componentType)];
    if (!size || !component) throw vrmError("vrm.errors.invalidAccessor");
    let length = count * size * component;
    if (accessor.bufferView !== undefined) {
      const view = ref(views, accessor.bufferView);
      const stride = integer(view.byteStride ?? size * component, 252);
      if (stride < size * component) throw vrmError("vrm.errors.invalidStride");
      integer(Number(accessor.byteOffset ?? 0) + Math.max(0, count - 1) * stride + (count ? size * component : 0), Number(view.byteLength));
      // sparse は interleaved array 全体を複製する。共有 view でもアクセサごとに計上する。
      length += count * stride;
    }
    if (accessor.sparse !== undefined) {
      length *= 2;
      const sparse = object(accessor.sparse);
      const sparseCount = integer(sparse.count, count);
      const indices = object(sparse.indices);
      const values = object(sparse.values);
      const indexBytes = components[String(indices.componentType)];
      if (![5121, 5123, 5125].includes(Number(indices.componentType)) || !indexBytes) throw vrmError("vrm.errors.invalidSparse");
      integer(Number(indices.byteOffset ?? 0) + sparseCount * indexBytes, Number(ref(views, indices.bufferView).byteLength));
      integer(Number(values.byteOffset ?? 0) + sparseCount * size * component, Number(ref(views, values.bufferView).byteLength));
    }
    return length;
  });
  integer(bytes.reduce((sum, value) => sum + value, 0), MEMORY_LIMIT);
  if (list(json.animations).length) throw vrmError("vrm.errors.animationsUnsupported");
  const nodes = table(json.nodes, 1024);
  const parents = new Map<number, number>();
  nodes.forEach((node, parent) => {
    for (const child of list(node.children, 1024)) {
      const index = integer(child, nodes.length - 1);
      if (parents.has(index)) throw vrmError("vrm.errors.duplicateNode");
      parents.set(index, parent);
    }
  });
  nodes.forEach((_, index) => {
    const seen = new Set<number>();
    let current: number | undefined = index;
    while (current !== undefined) {
      if (seen.has(current) || seen.size >= 64) throw vrmError("vrm.errors.hierarchyCycle");
      seen.add(current);
      current = parents.get(current);
    }
  });
  const scenes = table(json.scenes, 1);
  if (json.scene !== undefined) ref(scenes, json.scene);
  for (const scene of scenes) {
    const roots = new Set<number>();
    for (const root of list(scene.nodes, 1024)) {
      const index = integer(root, nodes.length - 1);
      if (roots.has(index) || parents.has(index)) throw vrmError("vrm.errors.duplicateSceneRef");
      roots.add(index);
    }
  }
  const materials = table(json.materials, 64);
  const meshes = table(json.meshes, 128);
  const meshCosts = meshes.map((mesh) => {
    let memory = 0;
    let vertices = 0;
    const primitives = table(mesh.primitives, 64);
    for (const primitive of primitives) {
      const attributes = object(primitive.attributes);
      const position = ref(accessors, attributes.POSITION);
      const count = Number(position.count);
      for (const index of Object.values(attributes)) memory += ref(bytes, index);
      vertices += primitive.indices === undefined ? count : Number(ref(accessors, primitive.indices).count);
      if (primitive.material !== undefined) ref(materials, primitive.material);
      const targets = table(primitive.targets, 256);
      // GPU のモーフ用テクスチャ（RGBA float、3属性、行端の余白を含む）。
      memory += (count * 3 + 4096) * 16 * targets.length;
      for (const target of targets) for (const index of Object.values(target)) memory += ref(bytes, index);
    }
    return { memory, vertices, draws: primitives.length };
  });
  let memory = 0;
  let vertices = 0;
  let draws = 0;
  for (const cost of meshCosts) memory += cost.memory;
  const skins = table(json.skins, 128);
  for (const skin of skins) {
    for (const joint of list(skin.joints, 256)) ref(nodes, joint);
    if (skin.skeleton !== undefined) ref(nodes, skin.skeleton);
    if (skin.inverseBindMatrices !== undefined) ref(accessors, skin.inverseBindMatrices);
  }
  for (const node of nodes) {
    if (node.skin !== undefined) ref(skins, node.skin);
    if (node.mesh === undefined) continue;
    const cost = ref(meshCosts, node.mesh);
    memory += cost.memory;
    vertices += cost.vertices;
    draws += cost.draws;
  }
  integer(memory, MEMORY_LIMIT * 2);
  integer(vertices, 2_000_000);
  integer(draws, 512);
  const images = table(json.images, 32);
  for (const image of images) {
    ref(views, image.bufferView);
    if (image.mimeType !== "image/png" && image.mimeType !== "image/jpeg") throw vrmError("vrm.errors.unsupportedImage");
  }
  const samplers = table(json.samplers, 64);
  for (const texture of table(json.textures, 64)) {
    ref(images, texture.source);
    if (texture.sampler !== undefined) ref(samplers, texture.sampler);
  }
  const extensions = object(json.extensions);
  let expressionCount = 0;
  let expressionBinds = 0;
  const checkExpression = (value: unknown, legacy: boolean): void => {
    const expression = object(value);
    expressionCount++;
    for (const entry of list(legacy ? expression.binds : expression.morphTargetBinds, 256)) {
      const bind = object(entry);
      if (bind.weight !== undefined && (typeof bind.weight !== "number" || bind.weight < 0 || bind.weight > (legacy ? 100 : 1))) throw vrmError("vrm.errors.invalidWeight");
      const mesh = legacy ? ref(meshes, bind.mesh) : ref(meshes, ref(nodes, bind.node).mesh);
      for (const primitive of table(mesh.primitives, 64)) integer(bind.index, list(primitive.targets, 256).length - 1);
      expressionBinds += Math.max(1, draws);
    }
    for (const entry of list(legacy ? expression.materialValues : expression.materialColorBinds, 256)) {
      const bind = object(entry);
      vector(bind.targetValue, 4);
      if (!legacy) {
        ref(materials, bind.material);
        if (!["color", "emissionColor", "shadeColor", "matcapColor", "rimColor", "outlineColor"].includes(String(bind.type))) throw vrmError("vrm.errors.invalidExpressionColor");
      } else if (typeof bind.materialName !== "string" || typeof bind.propertyName !== "string") throw vrmError("vrm.errors.invalidExpressionMaterial");
      expressionBinds += Math.max(1, draws);
    }
    // Texture.clone を作るバインドは画像予算も別途計上する。
    for (const entry of list(expression.textureTransformBinds, 256)) {
      const bind = object(entry);
      ref(materials, bind.material);
      if (bind.offset !== undefined) vector(bind.offset, 2);
      if (bind.scale !== undefined) vector(bind.scale, 2);
      expressionBinds += Math.max(1, draws);
    }
  };
  if (extensions.VRMC_vrm !== undefined) {
    const expressions = object(object(extensions.VRMC_vrm).expressions ?? {});
    for (const section of [expressions.preset, expressions.custom]) {
      for (const expression of Object.values(object(section ?? {}))) checkExpression(expression, false);
    }
  }
  if (extensions.VRM !== undefined) {
    const master = object(object(extensions.VRM).blendShapeMaster ?? {});
    for (const expression of list(master.blendShapeGroups, 64)) checkExpression(expression, true);
  }
  integer(expressionCount, 64);
  integer(expressionBinds, 4096);
  let joints = 0;
  let interactions = 0;
  if (extensions.VRM !== undefined) {
    const secondary = object(object(extensions.VRM).secondaryAnimation ?? {});
    const groups = table(secondary.colliderGroups, 256);
    const colliders = groups.map((group) => list(group.colliders, 256).length);
    for (const group of table(secondary.boneGroups, 256)) {
      const expanded = list(group.bones, 1024).length * nodes.length;
      const collisionCount = list(group.colliderGroups, 256).reduce<number>((sum, index) => sum + ref(colliders, index), 0);
      joints += expanded;
      interactions += expanded * Math.max(1, collisionCount);
    }
  }
  if (extensions.VRMC_springBone !== undefined) {
    const spring = object(extensions.VRMC_springBone);
    const colliders = table(spring.colliders, 256);
    const groups = table(spring.colliderGroups, 256).map((group) => {
      const references = list(group.colliders, 256);
      for (const index of references) ref(colliders, index);
      return references.length;
    });
    for (const item of table(spring.springs, 256)) {
      const count = list(item.joints, 256).length;
      const collisionCount = list(item.colliderGroups, 256).reduce<number>((sum, index) => sum + ref(groups, index), 0);
      joints += count;
      interactions += count * Math.max(1, collisionCount);
    }
  }
  integer(joints, 4096);
  integer(interactions, 65536);
}
