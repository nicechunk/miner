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
    const detectionInput = vector.profile === "forged_item"
      ? Buffer.from(`NCF1.${input.toString("base64url")}`)
      : input;
    const detected = JSON.parse(wasmModule.detect_input_json(detectionInput));
    assertEqual(detected.profile, vector.profile, `${vector.file} detected profile`);
    assertEqual(detected.format, {
      terrain_delta: "chunkbroken-v1",
      building: "ncm3-v1",
      forged_item: "ncf1-v15",
    }[vector.profile], `${vector.file} detected format`);
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

    const { stdout: nativeNcm4Stdout } = await exec(binary, [
      "--json",
      "ncm4",
      "analyze",
      inputPath,
      "--profile",
      vector.profile,
    ], { cwd: root });
    const nativeNcm4 = JSON.parse(nativeNcm4Stdout);
    const wasmNcm4 = JSON.parse(wasmModule.ncm4_analyze_json(vector.profile, input));
    assertEqual(wasmNcm4.semanticRoot, nativeNcm4.semanticRoot, `${vector.file} NCM4 semantic root`);
    assertEqual(wasmNcm4.encodingHash, nativeNcm4.encodingHash, `${vector.file} NCM4 encoding hash`);
    assertEqual(wasmNcm4.ncm4TotalBytes, nativeNcm4.ncm4TotalBytes, `${vector.file} NCM4 total bytes`);
    assertEqual(wasmNcm4.fixedHeaderBytes, nativeNcm4.fixedHeaderBytes, `${vector.file} NCM4 fixed header`);
    assertEqual(wasmNcm4.profileHeaderBytes, nativeNcm4.profileHeaderBytes, `${vector.file} NCM4 profile header`);
    assertEqual(wasmNcm4.bodyBytes, nativeNcm4.bodyBytes, `${vector.file} NCM4 body bytes`);
    assertEqual(wasmNcm4.residualBytes, nativeNcm4.residualBytes, `${vector.file} NCM4 residual bytes`);
    assertEqual(wasmNcm4.patches, nativeNcm4.patches, `${vector.file} NCM4 patches`);
    assertEqual(wasmNcm4.decodeUnits, nativeNcm4.decodeUnits, `${vector.file} NCM4 decode units`);
    assertEqual(wasmNcm4.exact, true, `${vector.file} NCM4 exact`);
    const candidate = Buffer.from(wasmNcm4.candidateBase64, "base64");
    const decodedNcm4 = JSON.parse(wasmModule.decode_ncm4_json(candidate));
    assertEqual(decodedNcm4.semanticRoot, vector.semanticRoot, `${vector.file} decoded NCM4 root`);
    assertEqual(decodedNcm4.stats.totalBytes, wasmNcm4.ncm4TotalBytes, `${vector.file} decoded NCM4 bytes`);
    const verifiedNcm4 = JSON.parse(wasmModule.verify_ncm4_json(vector.profile, input, candidate));
    assertEqual(verifiedNcm4.exact, true, `${vector.file} verified NCM4 exact`);
    assertEqual(verifiedNcm4.mismatchCount, 0, `${vector.file} verified NCM4 mismatches`);
  }
} finally {
  await rm(temporary, { recursive: true, force: true });
}

console.log(`Native/WASM consistency passed for ${golden.vectors.length} vectors`);

function assertEqual(actual, expected, label) {
  if (actual !== expected) throw new Error(`${label}: expected ${expected}, received ${actual}`);
}
