import { execFile } from "node:child_process";
import { readdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const exec = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = resolve(root, "target", "debug", process.platform === "win32" ? "nicechunk-miner.exe" : "nicechunk-miner");
const vectorsRoot = resolve(root, "test-vectors");
const profiles = ["terrain_delta", "building", "forged_item"];

const { stdout: benchmarkText } = await exec(binary, ["--json", "benchmark", "--corpus", vectorsRoot], { cwd: root });
const benchmark = JSON.parse(benchmarkText);
const metrics = new Map(benchmark.vectors.map((item) => [resolve(root, item.file), item]));
const vectors = [];

for (const profile of profiles) {
  const directory = resolve(vectorsRoot, profile);
  const files = (await readdir(directory)).sort();
  for (const file of files) {
    const path = resolve(directory, file);
    const { stdout } = await exec(binary, ["--json", "inspect", path, "--profile", profile], { cwd: root });
    const inspected = JSON.parse(stdout);
    const baseline = { ...metrics.get(path), file: `${profile}/${file}` };
    vectors.push({
      profile,
      file: `${profile}/${file}`,
      incumbentBytes: inspected.inputBytes,
      semanticRoot: inspected.semanticRoot,
      encodingHash: inspected.encodingHash,
      voxelCount: inspected.voxelCount,
      baseline,
    });
  }
}

const manifest = {
  format: "nicechunk-pouw-golden-v1",
  protocolVersion: 1,
  vmVersion: 1,
  costModelVersion: 1,
  vectors,
};
await writeFile(resolve(vectorsRoot, "golden.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Generated ${vectors.length} golden vectors`);
