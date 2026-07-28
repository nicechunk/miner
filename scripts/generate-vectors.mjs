import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const chunkJsRoot = resolve(process.env.NICECHUNK_CHUNK_JS_ROOT || resolve(root, "..", "chunk.js"));
const blueprintCodec = await import(pathToFileURL(resolve(chunkJsRoot, "ncm", "blueprint-codec.js")).href);
const forgeCodec = await import(pathToFileURL(resolve(chunkJsRoot, "forge", "forge-core.js")).href);
const { createBlueprint, encodeNcm3 } = blueprintCodec;
const { createForgeDesign, encodeNcf1Bytes, forgeVoxelIndex } = forgeCodec;
const vectors = resolve(root, "test-vectors");

await Promise.all([
  mkdir(resolve(vectors, "terrain_delta"), { recursive: true }),
  mkdir(resolve(vectors, "building"), { recursive: true }),
  mkdir(resolve(vectors, "forged_item"), { recursive: true }),
]);

function chunkBroken(coords, { minY = -64, capacity = 64 } = {}) {
  if (coords.length > capacity) throw new Error("ChunkBroken fixture exceeds capacity");
  const bytes = new Uint8Array(16 + capacity * 3);
  bytes.set(new TextEncoder().encode("NCBK"));
  bytes[4] = 1;
  new DataView(bytes.buffer).setUint16(6, coords.length, true);
  new DataView(bytes.buffer).setUint16(8, capacity, true);
  new DataView(bytes.buffer).setInt16(10, minY, true);
  coords.forEach(({ x, y, z }, index) => {
    const packed = x | (z << 4) | (y << 8);
    const offset = 16 + index * 3;
    bytes[offset] = packed & 0xff;
    bytes[offset + 1] = (packed >>> 8) & 0xff;
    bytes[offset + 2] = (packed >>> 16) & 0xff;
  });
  return bytes;
}

const terrainNormal = Array.from({ length: 16 }, (_, x) => ({ x, y: 12, z: 3 }));
const terrainBoundary = [
  { x: 0, y: 0, z: 0 },
  { x: 15, y: 0, z: 15 },
  { x: 0, y: 511, z: 15 },
  { x: 15, y: 511, z: 0 },
];
const terrainComplex = [];
for (let y = 40; y < 44; y += 1) {
  for (let z = 3; z < 11; z += 1) {
    for (let x = 2; x < 10; x += 1) {
      if ((x + y + z) % 11 !== 0) terrainComplex.push({ x, y, z });
    }
  }
}

await writeFile(resolve(vectors, "terrain_delta", "normal-row.ncbk"), chunkBroken(terrainNormal));
await writeFile(resolve(vectors, "terrain_delta", "boundary.ncbk"), chunkBroken(terrainBoundary, { minY: -256 }));
await writeFile(resolve(vectors, "terrain_delta", "complex-cavity.ncbk"), chunkBroken(terrainComplex, { minY: -128, capacity: 256 }));

const buildingNormal = createBlueprint({ x: 8, y: 8, z: 8 }, "Normal box")
  .box(1, 0, 0, 0, 8, 4, 8);
const buildingBoundary = createBlueprint({ x: 256, y: 16, z: 256 }, "Boundary")
  .box(2, 255, 15, 255, 1, 1, 1)
  .repeat(3, 0, 0, 0, 1, 1, 1, 4, 85, 0, 85);
const buildingComplex = createBlueprint({ x: 32, y: 24, z: 32 }, "PoUW cottage")
  .box(2, 0, 0, 0, 32, 1, 32)
  .box(1, 4, 1, 4, 24, 8, 1)
  .box(1, 4, 1, 27, 24, 8, 1)
  .repeat(3, 4, 1, 5, 1, 8, 1, 4, 7, 0, 0)
  .gableFill(4, 8, 9, 10, 16, 12)
  .fence(5, 2, 1, 2, 20, 0, 4)
  .tree(5, 6, 6, 1, 20, 8, 3);

await writeFile(resolve(vectors, "building", "normal-box.ncm3"), `${encodeNcm3(buildingNormal)}\n`);
await writeFile(resolve(vectors, "building", "boundary.ncm3"), `${encodeNcm3(buildingBoundary)}\n`);
await writeFile(resolve(vectors, "building", "complex-cottage.ncm3"), `${encodeNcm3(buildingComplex)}\n`);

const equipment = {
  mass5g: 12,
  volumeMm3: 3456,
  attributes6: Array.from({ length: 12 }, (_, index) => index),
};
const full = new Uint8Array(14 * 10 * 14).fill(1);
const sparse = new Uint8Array(14 * 10 * 14);
sparse[forgeVoxelIndex(0, 0, 0)] = 1;
sparse[forgeVoxelIndex(13, 9, 13)] = 1;
const complex = new Uint8Array(full);
for (let z = 4; z < 10; z += 1) {
  for (let y = 2; y < 8; y += 1) {
    for (let x = 4; x < 10; x += 1) complex[forgeVoxelIndex(x, y, z)] = 0;
  }
}

const forgedNormal = createForgeDesign({
  equipment,
  components: [{ resource: 0, dimsQ: [64, 32, 64], offsetQ: [0, 0, 0], solid: full }],
});
const forgedBoundary = createForgeDesign({
  equipment: { ...equipment, mass5g: 65535, volumeMm3: 8191 * 16 ** 3 },
  components: [{
    resource: 5,
    color444: 0xfff,
    dimsQ: [1, 255, 1],
    offsetQ: [-512, 511, -512],
    solid: sparse,
    grip: { offsetQ: [-512, 511, -512], axis: 2, sign: -1, rotation: 3 },
  }],
});
const forgedComplex = createForgeDesign({
  equipment: { ...equipment, mass5g: 240, volumeMm3: 654321 },
  components: [{
    resource: 1,
    color444: 0xb64,
    dimsQ: [128, 80, 128],
    offsetQ: [0, 0, 0],
    solid: complex,
    grip: { offsetQ: [0, 320, 0], axis: 1, sign: 1, rotation: 1 },
    paintQuads: [{ axis: 1, side: 1, plane: 10, u0: 0, u1: 14, v0: 0, v1: 14, color444: 0xf80 }],
  }],
});

await writeFile(resolve(vectors, "forged_item", "normal-full.ncf1"), encodeNcf1Bytes(forgedNormal));
await writeFile(resolve(vectors, "forged_item", "boundary.ncf1"), encodeNcf1Bytes(forgedBoundary));
await writeFile(resolve(vectors, "forged_item", "complex-painted-cavity.ncf1"), encodeNcf1Bytes(forgedComplex));

await writeFile(resolve(vectors, "README.md"), `# NiceChunk PoUW v1 test vectors

Each profile has distinct normal, boundary, and complex fixtures generated by
the current source-of-truth JavaScript codec. Binary golden roots and metrics
are produced by the Rust verifier in \`golden.json\` during the test build.
`);

console.log(`Generated NiceChunk PoUW vectors in ${vectors}`);
