import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { access, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { build } from "esbuild";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "web", "dist");
const pinnedChunkJsRoot = resolve(root, ".dependencies", "chunk.js");
const chunkJsRoot = resolve(
  process.env.NICECHUNK_CHUNK_JS_ROOT
    || (existsSync(resolve(pinnedChunkJsRoot, "forge", "forge-core.js"))
      ? pinnedChunkJsRoot
      : resolve(root, "..", "chunk.js")),
);
const pinnedGameRoot = resolve(root, ".dependencies", "game");
const gameRoot = resolve(
  process.env.NICECHUNK_GAME_ROOT
    || (existsSync(resolve(pinnedGameRoot, "play", "play-chain-chunk-deltas.js"))
      ? pinnedGameRoot
      : resolve(root, "..", "game")),
);
const ncmCodecPath = resolve(chunkJsRoot, "ncm", "blueprint-codec.js");
const forgeCodecPath = resolve(chunkJsRoot, "forge", "forge-core.js");
const terrainSourcePath = resolve(gameRoot, "play", "play-chain-chunk-deltas.js");
const temporary = await mkdtemp(resolve(tmpdir(), "nicechunk-pouw-js-codec-"));

try {
  await Promise.all([
    access(ncmCodecPath),
    access(forgeCodecPath),
    access(terrainSourcePath),
  ]);
  const { decodeNcm3, expandBlueprint } = await import(pathToFileURL(ncmCodecPath));
  const { decodeNcf1, encodeForgeVolumeMm3 } = await import(pathToFileURL(forgeCodecPath));

  const terrainModulePath = resolve(temporary, "terrain-decoder.cjs");
  const terrainSource = await readFile(terrainSourcePath, "utf8");
  await build({
    stdin: {
      // The production decoder is intentionally private. Export it only from
      // this in-memory test entry so compatibility tests exercise the pinned
      // Game implementation without changing or copying that repository.
      contents: `${terrainSource}\nexport { decodeChunkBrokenDeltas };\n`,
      loader: "js",
      resolveDir: dirname(terrainSourcePath),
      sourcefile: terrainSourcePath,
    },
    bundle: true,
    format: "cjs",
    platform: "node",
    target: "node20",
    outfile: terrainModulePath,
    logLevel: "silent",
    treeShaking: true,
    plugins: [{
      name: "pinned-chunk-js-runtime",
      setup(context) {
        context.onResolve({ filter: /^\/chunk\.js\// }, ({ path }) => ({
          path: resolve(chunkJsRoot, path.slice("/chunk.js/".length)),
        }));
      },
    }],
  });
  const { decodeChunkBrokenDeltas } = await import(pathToFileURL(terrainModulePath));

  const manifest = JSON.parse(await readFile(resolve(dist, "asset-manifest.json"), "utf8"));
  const wasm = await import(pathToFileURL(resolve(dist, manifest.assets.wasmGlue)));
  wasm.initSync({ module: await readFile(resolve(dist, manifest.assets.wasm)) });
  const golden = JSON.parse(await readFile(resolve(root, "test-vectors", "golden.json"), "utf8"));

  for (const vector of golden.vectors) {
    const input = await readFile(resolve(root, "test-vectors", vector.file));
    const rust = JSON.parse(wasm.inspect_json(vector.profile, input)).semantics;
    let javascript;
    if (vector.profile === "terrain_delta") {
      const minY = input.readInt16LE(10);
      javascript = normalizeTerrain(decodeChunkBrokenDeltas(input, 0, 0, 16, minY), minY);
    } else if (vector.profile === "building") {
      javascript = normalizeBuilding(decodeNcm3(input.toString("utf8").trim()), expandBlueprint);
    } else if (vector.profile === "forged_item") {
      javascript = normalizeForged(decodeNcf1(input, { requireCanonical: true }), encodeForgeVolumeMm3);
    } else {
      throw new Error(`Unsupported compatibility profile ${vector.profile}`);
    }
    assert.deepEqual(javascript, rust, `${vector.file} differs from the source JavaScript codec`);
  }

  console.log(`Current ChunkBroken/NCM3/NCF1 JavaScript codec compatibility passed for ${golden.vectors.length} vectors`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}

function normalizeTerrain(deltas, minY) {
  const deleted = deltas
    .map((delta) => ({ x: delta.worldX, y: delta.worldY - minY, z: delta.worldZ }))
    .sort((left, right) => terrainId(left) - terrainId(right));
  return {
    profile: "terrain_delta",
    semantics: { deleted, minY },
  };
}

function normalizeBuilding(blueprint, expandBlueprint) {
  const size = [blueprint.size.x, blueprint.size.y, blueprint.size.z];
  const cells = new Map();
  for (const cuboid of expandBlueprint(blueprint)) {
    for (let y = cuboid.y; y < cuboid.y + cuboid.h; y += 1) {
      for (let z = cuboid.z; z < cuboid.z + cuboid.d; z += 1) {
        for (let x = cuboid.x; x < cuboid.x + cuboid.w; x += 1) {
          cells.set(buildingId(size, x, y, z), { material: cuboid.material, x, y, z });
        }
      }
    }
  }
  const voxels = [...cells.entries()]
    .sort(([left], [right]) => left - right)
    .map(([, voxel]) => voxel);
  return {
    profile: "building",
    semantics: { size, voxels },
  };
}

function normalizeForged(design, encodeForgeVolumeMm3) {
  const equipment = {
    attributes6: Array.from(design.equipment.attributes6),
    encodedVolume: encodeForgeVolumeMm3(design.equipment.volumeMm3),
    mass5g: design.equipment.mass5g,
  };
  const geometry = design.components
    ? {
        components: design.components.map((component) => ({
          color444: component.color444,
          dimensionsQ: component.dimsQ,
          grip: normalizeGrip(component.grip),
          offsetQ: component.offsetQ,
          paint: component.paintQuads.map(normalizeQuad),
          resource: component.resource,
          solid: Array.from(component.solid)
            .flatMap((occupied, cell) => (occupied ? [cell] : [])),
        })),
        mode: "components",
      }
    : {
        appearance: {
          dimensionsQ: design.appearance.dimsQ,
          grip: normalizeGrip(design.appearance.grip),
          quads: design.appearance.quads.map(normalizeQuad),
        },
        mode: "appearance",
      };
  return {
    profile: "forged_item",
    semantics: { equipment, geometry },
  };
}

function normalizeGrip(grip) {
  if (!grip) return null;
  return {
    axis: grip.axis,
    offsetQ: grip.offsetQ,
    rotation: grip.rotation,
    sign: grip.sign,
  };
}

function normalizeQuad(quad) {
  return {
    axis: quad.axis,
    color444: quad.color444,
    plane: quad.plane,
    side: quad.side,
    u0: quad.u0,
    u1: quad.u1,
    v0: quad.v0,
    v1: quad.v1,
    ...("resource" in quad ? { resource: quad.resource } : {}),
  };
}

function terrainId(coord) {
  return coord.x + 16 * (coord.z + 16 * coord.y);
}

function buildingId(size, x, y, z) {
  return x + size[0] * (z + size[2] * y);
}
