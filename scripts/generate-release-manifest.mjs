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
  message: "Every listed archive passed its native self-test before publication.",
};
await mkdir(dirname(output), { recursive: true });
await writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`);

if (options.notes) {
  const notes = `# NiceChunk Proof of Useful Work Miner ${version}\n\n`
    + `- Commit: \`${commit}\`\n`
    + "- Protocol version: `1`\n"
    + "- VM version: `1`\n"
    + "- Cost model version: `1`\n\n"
    + "All archives were built on their native GitHub-hosted runner, passed `nicechunk-miner self-test`, and include binary SHA-256 values in `SHA256SUMS`. Archive hashes are published as adjacent `.sha256` assets and in `release-manifest.json`.\n\n"
    + "Compatibility: ChunkBroken v1, NCM3 v1, and complete NCF1 v15 import. This release is a native/WASM research preview and does not submit transactions or issue rewards.\n";
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
