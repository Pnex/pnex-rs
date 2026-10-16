// A tiny rigged GLB built in memory: a column skinned on two bones ("Root",
// "Elbow") with one animation ("Wave"). Used to test the 3D viewer of an
// object's gallery (assisted-creation.md D231) without a binary fixture.

const SEGMENTS = 8;
const RINGS = 9; // y = 0, 0.25 … 2
const RADIUS = 0.15;

function pad4(buf: Buffer, fill = 0): Buffer {
  const rest = (4 - (buf.length % 4)) % 4;
  return rest ? Buffer.concat([buf, Buffer.alloc(rest, fill)]) : buf;
}

export function rigGlb(): Buffer {
  const pos: number[] = [];
  const nor: number[] = [];
  const joints: number[] = [];
  const weights: number[] = [];
  for (let r = 0; r < RINGS; r++) {
    const y = r * 0.25;
    // Below 0.75 on the root, above 1.25 on the elbow, blended between.
    const w1 = Math.min(1, Math.max(0, (y - 0.75) / 0.5));
    for (let s = 0; s < SEGMENTS; s++) {
      const a = (s / SEGMENTS) * Math.PI * 2;
      pos.push(Math.cos(a) * RADIUS, y, Math.sin(a) * RADIUS);
      nor.push(Math.cos(a), 0, Math.sin(a));
      joints.push(0, 1, 0, 0);
      weights.push(1 - w1, w1, 0, 0);
    }
  }
  const idx: number[] = [];
  for (let r = 0; r < RINGS - 1; r++) {
    for (let s = 0; s < SEGMENTS; s++) {
      const a = r * SEGMENTS + s;
      const b = r * SEGMENTS + ((s + 1) % SEGMENTS);
      const c = a + SEGMENTS;
      const d = b + SEGMENTS;
      idx.push(a, c, b, b, c, d);
    }
  }
  // Inverse bind matrices, column-major: identity, then translate(0, -1, 0).
  const ibm = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1];
  const half = Math.sin(Math.PI / 6); // 60° about Z
  const times = [0, 1, 2];
  const rots = [0, 0, 0, 1, 0, 0, half, Math.cos(Math.PI / 6), 0, 0, 0, 1];

  const chunks: Buffer[] = [];
  const views: object[] = [];
  const add = (b: Buffer, target?: number) => {
    const offset = chunks.reduce((n, c) => n + c.length, 0);
    chunks.push(pad4(b));
    views.push({ buffer: 0, byteOffset: offset, byteLength: b.length, ...(target ? { target } : {}) });
    return views.length - 1;
  };
  const f32 = (a: number[]) => Buffer.from(new Float32Array(a).buffer);
  const u16 = (a: number[]) => Buffer.from(new Uint16Array(a).buffer);
  const vPos = add(f32(pos), 34962);
  const vNor = add(f32(nor), 34962);
  const vJoi = add(u16(joints), 34962);
  const vWei = add(f32(weights), 34962);
  const vIdx = add(u16(idx), 34963);
  const vIbm = add(f32(ibm));
  const vTim = add(f32(times));
  const vRot = add(f32(rots));
  const n = RINGS * SEGMENTS;
  const json = {
    asset: { version: '2.0', generator: 'pnex-e2e' },
    scene: 0,
    scenes: [{ nodes: [0, 2] }],
    nodes: [
      { name: 'Root', children: [1] },
      { name: 'Elbow', translation: [0, 1, 0] },
      { name: 'Arm', mesh: 0, skin: 0 },
    ],
    meshes: [
      {
        primitives: [
          {
            attributes: { POSITION: 0, NORMAL: 1, JOINTS_0: 2, WEIGHTS_0: 3 },
            indices: 4,
            material: 0,
          },
        ],
      },
    ],
    materials: [{ pbrMetallicRoughness: { baseColorFactor: [0.1, 0.55, 0.65, 1], metallicFactor: 0, roughnessFactor: 0.6 } }],
    skins: [{ joints: [0, 1], inverseBindMatrices: 5, skeleton: 0 }],
    animations: [
      {
        name: 'Wave',
        samplers: [{ input: 6, output: 7, interpolation: 'LINEAR' }],
        channels: [{ sampler: 0, target: { node: 1, path: 'rotation' } }],
      },
    ],
    accessors: [
      { bufferView: vPos, componentType: 5126, count: n, type: 'VEC3', min: [-RADIUS, 0, -RADIUS], max: [RADIUS, 2, RADIUS] },
      { bufferView: vNor, componentType: 5126, count: n, type: 'VEC3' },
      { bufferView: vJoi, componentType: 5123, count: n, type: 'VEC4' },
      { bufferView: vWei, componentType: 5126, count: n, type: 'VEC4' },
      { bufferView: vIdx, componentType: 5123, count: idx.length, type: 'SCALAR' },
      { bufferView: vIbm, componentType: 5126, count: 2, type: 'MAT4' },
      { bufferView: vTim, componentType: 5126, count: 3, type: 'SCALAR', min: [0], max: [2] },
      { bufferView: vRot, componentType: 5126, count: 3, type: 'VEC4' },
    ],
    bufferViews: views,
    buffers: [{ byteLength: 0 }],
  };
  const bin = Buffer.concat(chunks);
  json.buffers[0].byteLength = bin.length;
  const jsonBuf = pad4(Buffer.from(JSON.stringify(json)), 0x20);
  const header = Buffer.alloc(12);
  header.write('glTF', 0);
  header.writeUInt32LE(2, 4);
  header.writeUInt32LE(12 + 8 + jsonBuf.length + 8 + bin.length, 8);
  const chunk = (type: string, data: Buffer) => {
    const h = Buffer.alloc(8);
    h.writeUInt32LE(data.length, 0);
    h.write(type, 4);
    return Buffer.concat([h, data]);
  };
  return Buffer.concat([header, chunk('JSON', jsonBuf), chunk('BIN\0', bin)]);
}
