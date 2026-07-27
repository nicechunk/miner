import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { promisify } from "node:util";

const exec = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "web", "dist");
const assets = JSON.parse(await readFile(resolve(dist, "asset-manifest.json"), "utf8")).assets;
const wasmModule = await import(pathToFileURL(resolve(dist, assets.wasmGlue)));
wasmModule.initSync({ module: await readFile(resolve(dist, assets.wasm)) });
const golden = JSON.parse(await readFile(resolve(root, "test-vectors", "golden.json"), "utf8"));
const binary = resolve(root, "target", "debug", process.platform === "win32" ? "nicechunk-miner.exe" : "nicechunk-miner");
const temporary = await mkdtemp(resolve(tmpdir(), "nicechunk-pouw-wasm-"));

try {
  for (const vector of golden.vectors) {
    const inputPath = resolve(root, "test-vectors", vector.file);
    const input = await readFile(inputPath);
    const inspected = JSON.parse(wasmModule.inspect_json(vector.profile, input));
    assertEqual(inspected.semanticRoot, vector.semanticRoot, `${vector.file} semantic root`);
    assertEqual(inspected.encodingHash, vector.encodingHash, `${vector.file} encoding hash`);
    assertEqual(inspected.incumbentBytes, vector.incumbentBytes, `${vector.file} incumbent bytes`);

    const taskPath = resolve(temporary, `${vector.profile}-${vector.file.replaceAll("/", "-")}.task`);
    const resultPath = `${taskPath}.result`;
    await exec(binary, ["--json", "task", "create", "--profile", vector.profile, "--input", inputPath, "--out", taskPath, "--asset-id", `wasm:${vector.file}`], { cwd: root });
    await exec(binary, ["--json", "baseline", "--task", taskPath, "--out", resultPath], { cwd: root });
    const { stdout } = await exec(binary, ["--json", "verify", "--task", taskPath, "--result", resultPath], { cwd: root }).catch((error) => {
      // Exact but non-smaller candidates intentionally use exit code 4; stdout still contains the report.
      if (error.code === 4 && error.stdout) return error;
      throw error;
    });
    const native = JSON.parse(stdout);
    const wasm = JSON.parse(wasmModule.baseline_json(vector.profile, input));
    assertEqual(wasm.candidateSemanticRoot, native.candidateSemanticRoot, `${vector.file} candidate root`);
    assertEqual(wasm.candidateEncodingHash, native.candidateEncodingHash, `${vector.file} candidate encoding hash`);
    assertEqual(wasm.candidateBytes, native.candidateBytes, `${vector.file} candidate bytes`);
    assertEqual(wasm.programBytes, native.programBytes, `${vector.file} program bytes`);
    assertEqual(wasm.residualBytes, native.residualBytes, `${vector.file} residual bytes`);
    assertEqual(wasm.decodeUnits, native.decodeUnits, `${vector.file} decode units`);
    assertEqual(wasm.mismatchCount, 0, `${vector.file} mismatch count`);
    assertEqual(wasm.exact, true, `${vector.file} exact`);
  }
} finally {
  await rm(temporary, { recursive: true, force: true });
}

console.log(`Native/WASM consistency passed for ${golden.vectors.length} vectors`);

function assertEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`${label}: expected ${expected}, received ${actual}`);
}
