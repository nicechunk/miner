import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cp, lstat, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArgs(process.argv.slice(2));
const dist = resolve(options.dist || resolve(root, "web", "dist"));
const output = resolve(options.output || resolve(root, "artifacts", "web"));
const version = options.version || JSON.parse(await readFile(resolve(root, "package.json"), "utf8")).version;
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(version)) throw new Error("Invalid release version");

const sourceFiles = await walk(dist);
if (!sourceFiles.some((path) => relative(dist, path) === "index.html")) throw new Error("Web dist has no index.html");
if (!sourceFiles.some((path) => relative(dist, path) === "release-manifest.json")) throw new Error("Web dist has no release-manifest.json");
const fileRecords = [];
for (const path of sourceFiles) {
  const name = relative(dist, path).split(sep).join("/");
  const bytes = await readFile(path);
  fileRecords.push({ path: name, bytes: bytes.length, sha256: sha256(bytes) });
}
fileRecords.sort((left, right) => left.path.localeCompare(right.path));
const fileManifest = `${JSON.stringify({ format: "nicechunk-miner-web-release-v1", version, files: fileRecords }, null, 2)}\n`;
const manifestDigest = sha256(Buffer.from(fileManifest));
const releaseId = `miner-${version}-${manifestDigest.slice(0, 16)}`;
const releaseDirectory = resolve(output, releaseId);
await rm(releaseDirectory, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await cp(dist, releaseDirectory, { recursive: true, dereference: false, errorOnExist: true });
await writeFile(resolve(releaseDirectory, "MANIFEST.json"), fileManifest);

const archive = resolve(output, `${releaseId}.tar.gz`);
await rm(archive, { force: true });
execFileSync("tar", [
  "--sort=name",
  "--mtime=@0",
  "--owner=0",
  "--group=0",
  "--numeric-owner",
  "--format=gnu",
  "-czf", archive,
  "-C", output,
  releaseId,
], { stdio: "inherit" });
const archiveDigest = sha256(await readFile(archive));
await writeFile(resolve(output, `${releaseId}.sha256`), `${archiveDigest}  ${releaseId}.tar.gz\n`);

console.log(JSON.stringify({
  releaseId,
  manifestDigest,
  archive,
  archiveDigest,
  fileCount: fileRecords.length,
  bytes: fileRecords.reduce((sum, file) => sum + file.bytes, 0),
}));

async function walk(path) {
  const output = [];
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = resolve(path, entry.name);
    const metadata = await lstat(child);
    if (metadata.isSymbolicLink()) throw new Error(`Release input contains symlink: ${child}`);
    if (metadata.isDirectory()) output.push(...await walk(child));
    else if (metadata.isFile()) output.push(child);
    else throw new Error(`Release input contains a special file: ${child}`);
  }
  return output;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function parseArgs(args) {
  const result = {};
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index]?.replace(/^--/u, "");
    const value = args[index + 1];
    if (!key || value == null) throw new Error("Expected --key value arguments");
    result[key] = value;
  }
  return result;
}
