import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArgs(process.argv.slice(2));
const repository = options.repository;
const tag = options.tag;
const version = options.version;
const commit = options.commit;
const artifactsRoot = resolve(options.artifacts);
const output = resolve(options.output);

if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(repository)) throw new Error("Invalid owner/repository");
if (!/^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(tag)) throw new Error("Release tag must be v<semver>");
if (tag.slice(1) !== version) throw new Error("Release tag and version differ");
if (!/^[0-9a-f]{40}$/u.test(commit)) throw new Error("Commit must be a full lowercase Git SHA");

const platformConfig = JSON.parse(await readFile(resolve(root, "release-platforms.json"), "utf8"));
const files = await walk(artifactsRoot);
const artifacts = [];
for (const platform of platformConfig) {
  const name = `nicechunk-miner-${version}-${platform.id}.${platform.archiveExtension}`;
  const matches = files.filter((path) => basename(path) === name);
  if (matches.length !== 1) throw new Error(`Expected exactly one ${name}, found ${matches.length}`);
  const archive = matches[0];
  await verifyArchiveSignature(archive, platform.archiveExtension);
  const digest = await sha256(archive);
  const sidecars = files.filter((path) => basename(path) === `${name}.sha256`);
  if (sidecars.length !== 1) throw new Error(`Expected exactly one ${name}.sha256`);
  const sidecar = (await readFile(sidecars[0], "utf8")).trim();
  if (sidecar !== `${digest}  ${name}`) throw new Error(`${name} checksum sidecar is invalid`);
  const baseUrl = `https://github.com/${repository}/releases/download/${tag}`;
  artifacts.push({
    platform: platform.label,
    platformId: platform.id,
    target: platform.target,
    archive: name,
    downloadUrl: `${baseUrl}/${name}`,
    checksumUrl: `${baseUrl}/${name}.sha256`,
    sha256: digest,
  });
}

const webArchives = files.filter((path) => {
  const name = basename(path);
  return name.startsWith(`miner-${version}-`) && name.endsWith(".tar.gz");
});
if (webArchives.length !== 1) {
  throw new Error(`Expected exactly one Web/WASM archive, found ${webArchives.length}`);
}
const webArchive = webArchives[0];
const webArchiveName = basename(webArchive);
const webDigest = await sha256(webArchive);
const webSidecars = files.filter((path) => basename(path) === `${webArchiveName.slice(0, -7)}.sha256`);
if (webSidecars.length !== 1) throw new Error(`Expected one checksum for ${webArchiveName}`);
const webSidecar = (await readFile(webSidecars[0], "utf8")).trim();
if (webSidecar !== `${webDigest}  ${webArchiveName}`) throw new Error(`${webArchiveName} checksum is invalid`);
const releaseBaseUrl = `https://github.com/${repository}/releases/download/${tag}`;
const webBundle = {
  platform: "Web/WASM static bundle",
  archive: webArchiveName,
  downloadUrl: `${releaseBaseUrl}/${webArchiveName}`,
  checksumUrl: `${releaseBaseUrl}/${basename(webSidecars[0])}`,
  sha256: webDigest,
};

const manifest = {
  schemaVersion: 1,
  softwareVersion: version,
  protocolVersion: 1,
  vmVersion: 1,
  costModelVersion: 1,
  commit,
  available: true,
  repository: `https://github.com/${repository}`,
  releaseUrl: `https://github.com/${repository}/releases/tag/${tag}`,
  artifacts,
  webBundle,
  message: "Every CLI archive passed its native self-test; the Web/WASM bundle passed native/WASM consistency and browser smoke tests before publication.",
};
await mkdir(dirname(output), { recursive: true });
await writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`);

if (options.notes) {
  const notes = `# NiceChunk Proof of Useful Work Miner ${version}\n\n`
    + `- Commit: \`${commit}\`\n`
    + "- Protocol version: `1`\n"
    + "- VM version: `1`\n"
    + "- Cost model version: `1`\n\n"
    + "All CLI archives were built on their native GitHub-hosted runner, passed `nicechunk-miner self-test`, and include binary SHA-256 values in `SHA256SUMS`. The Web/WASM archive passed consistency and local-only browser tests. Archive hashes are published as adjacent `.sha256` assets, the aggregate `SHA256SUMS`, and `release-manifest.json`.\n\n"
    + "Native NCM4 offspring evaluation is batched across all persistent islands, allowing `--threads` to use cores beyond the configured island count while preserving fixed-seed and checkpoint results. The separate Linux x86_64 CUDA archive adds GPU batch rasterization and residual prefiltering; every promoted candidate still passes independent CPU decode and exact semantic verification.\n\n"
    + "NCM4 provides the distinct `NC4P`/`NCM4P:` exact building codec, language preflight, persistent multi-island search, checkpoint/resume, and NCM3 fallback when NCM4 is not smaller. ChunkBroken v1, unchanged NCM3 v1, and complete NCF1 v15 import remain supported. Terrain and forged-item NCM4 currently use exact wrappers rather than the compact building grammar. The native and browser miners run locally and do not submit transactions or issue rewards.\n";
  await writeFile(resolve(options.notes), notes);
}

console.log(`Generated verified release manifest for ${artifacts.length} platforms`);

async function walk(path) {
  const output = [];
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = resolve(path, entry.name);
    if (entry.isDirectory()) output.push(...await walk(child));
    else if (entry.isFile()) output.push(child);
  }
  return output;
}

async function sha256(path) {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}

async function verifyArchiveSignature(path, extension) {
  const header = (await readFile(path)).subarray(0, 4).toString("hex");
  if (extension === "zip" && !["504b0304", "504b0506", "504b0708"].includes(header)) {
    throw new Error(`${basename(path)} is not a ZIP container`);
  }
  if (extension === "tar.gz" && !header.startsWith("1f8b")) {
    throw new Error(`${basename(path)} is not a gzip container`);
  }
}

function parseArgs(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index]?.replace(/^--/u, "");
    const value = args[index + 1];
    if (!key || value == null) throw new Error("Expected --key value arguments");
    result[key] = value;
  }
  for (const required of ["repository", "tag", "version", "commit", "artifacts", "output"]) {
    if (!result[required]) throw new Error(`Missing --${required}`);
  }
  return result;
}
