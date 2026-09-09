// 第三者モデルを配布せず、実際のローダーを検証するための最小モデル。
export function createTestVrm(version: "0" | "1"): ArrayBuffer {
  const bones = [
    ["hips", -1, 0, 1, 0], ["spine", 0, 0, 0.2, 0], ["head", 1, 0, 0.5, 0],
    ["leftUpperLeg", 0, 0.1, -0.1, 0], ["leftLowerLeg", 3, 0, -0.4, 0], ["leftFoot", 4, 0, -0.4, 0],
    ["rightUpperLeg", 0, -0.1, -0.1, 0], ["rightLowerLeg", 6, 0, -0.4, 0], ["rightFoot", 7, 0, -0.4, 0],
    ["leftUpperArm", 1, 0.2, 0.3, 0], ["leftLowerArm", 9, 0.3, 0, 0], ["leftHand", 10, 0.2, 0, 0],
    ["rightUpperArm", 1, -0.2, 0.3, 0], ["rightLowerArm", 12, -0.3, 0, 0], ["rightHand", 13, -0.2, 0, 0],
  ] as const;
  const nodes = bones.map(([name, , x, y, z], index) => ({
    name, translation: [x, y, z], children: bones.flatMap((bone, child) => bone[1] === index ? [child] : []),
  }));
  const positions = new Float32Array([-0.5, 0, 0, 0.5, 0, 0, 0, 1.8, 0]);
  const extension = version === "1" ? "VRMC_vrm" : "VRM";
  const vrm = version === "1" ? {
    specVersion: "1.0", meta: { name: "Test", version: "1", authors: ["CooSenpAI tests"], licenseUrl: "https://vrm.dev/licenses/1.0/" },
    humanoid: { humanBones: Object.fromEntries(bones.map(([name], node) => [name, { node }])) },
  } : {
    specVersion: "0.0", meta: { title: "Test", author: "CooSenpAI tests", version: "1", allowedUserName: "Everyone", licenseName: "CC0" },
    humanoid: { humanBones: bones.map(([bone], node) => ({ bone, node, useDefaultValues: true })) },
  };
  const json = {
    asset: { version: "2.0" }, scene: 0, scenes: [{ nodes: [0, bones.length] }],
    nodes: [...nodes, { mesh: 0 }], extensionsUsed: [extension], extensions: { [extension]: vrm },
    buffers: [{ byteLength: positions.byteLength }],
    bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: positions.byteLength }],
    accessors: [{ bufferView: 0, componentType: 5126, count: 3, type: "VEC3", min: [-0.5, 0, 0], max: [0.5, 1.8, 0] }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0 }, material: 0 }] }],
    materials: [{ doubleSided: true, pbrMetallicRoughness: { baseColorFactor: [0.3, 0.6, 0.9, 1], metallicFactor: 0 } }],
  };
  return createTestGlb(json, new Uint8Array(positions.buffer));
}

export function createTestGlb(json: unknown, binary = new Uint8Array(0)): ArrayBuffer {
  const encoded = new TextEncoder().encode(JSON.stringify(json));
  const jsonLength = Math.ceil(encoded.length / 4) * 4;
  const binaryLength = Math.ceil(binary.byteLength / 4) * 4;
  const data = new ArrayBuffer(28 + jsonLength + binaryLength);
  const header = new DataView(data);
  [0x46546c67, 2, data.byteLength, jsonLength, 0x4e4f534a].forEach((value, i) => header.setUint32(i * 4, value, true));
  new Uint8Array(data, 20, jsonLength).fill(32);
  new Uint8Array(data, 20, encoded.length).set(encoded);
  header.setUint32(20 + jsonLength, binaryLength, true);
  header.setUint32(24 + jsonLength, 0x004e4942, true);
  new Uint8Array(data, 28 + jsonLength).set(binary);
  return data;
}

export function createTexturedTestVrm(image: Uint8Array, mimeType = "image/png"): ArrayBuffer {
  const original = createTestVrm("1");
  const length = new DataView(original).getUint32(12, true);
  const json = JSON.parse(new TextDecoder().decode(new Uint8Array(original, 20, length)));
  const geometry = new Uint8Array(original, 28 + length);
  const binary = new Uint8Array(geometry.length + image.length);
  binary.set(geometry);
  binary.set(image, geometry.length);
  json.buffers[0].byteLength = binary.length;
  json.bufferViews.push({ buffer: 0, byteOffset: geometry.length, byteLength: image.length });
  json.images = [{ bufferView: 1, mimeType }];
  json.textures = [{ source: 0 }];
  json.materials[0].pbrMetallicRoughness.baseColorTexture = { index: 0 };
  return createTestGlb(json, binary);
}
