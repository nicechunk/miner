import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, copyFile, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const options = parseArgs(process.argv.slice(2));
const platform = options.platform;
const version = options.version;
const binary = resolve(options.binary);
const output = resolve(options.output);

if (!/^[a-z0-9][a-z0-9._-]*$/u.test(platform)) throw new Error("Invalid release platform ID");
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(version)) throw new Error("Invalid semantic version");

const platforms = JSON.parse(await readFile(resolve(root, "release-platforms.json"), "utf8"));
const entry = platforms.find((item) => item.id === platform);
if (!entry) throw new Error(`Unknown release platform ${platform}`);

await mkdir(output, { recursive: true });
const stageName = `.stage-${platform}`;
const stage = resolve(output, stageName);
await rm(stage, { recursive: true, force: true });
await mkdir(resolve(stage, "docs"), { recursive: true });
const binaryName = platform.startsWith("windows-") ? "nicechunk-miner.exe" : "nicechunk-miner";
await copyFile(binary, resolve(stage, binaryName));
if (!platform.startsWith("windows-")) await chmod(resolve(stage, binaryName), 0o755);
await copyFile(resolve(root, "README.md"), resolve(stage, "README.md"));
await copyFile(resolve(root, "docs", "cli.md"), resolve(stage, "docs", "cli.md"));
await copyFile(resolve(root, "docs", "benchmarks.md"), resolve(stage, "docs", "benchmarks.md"));
await copyFile(resolve(root, "docs", "protocol.md"), resolve(stage, "docs", "protocol.md"));
await copyFile(resolve(root, "docs", "security.md"), resolve(stage, "docs", "security.md"));

const stagedFiles = [
  binaryName,
  "README.md",
  "docs/benchmarks.md",
  "docs/cli.md",
  "docs/protocol.md",
  "docs/security.md",
];
const checksums = [];
for (const file of stagedFiles) {
  checksums.push(`${await sha256(resolve(stage, file))}  ${file}`);
}
await writeFile(resolve(stage, "SHA256SUMS"), `${checksums.join("\n")}\n`);

const archiveName = `nicechunk-miner-${version}-${platform}.${entry.archiveExtension}`;
const archive = resolve(output, archiveName);
await rm(archive, { force: true });
if (entry.archiveExtension === "zip") {
  if (process.platform !== "win32") throw new Error("ZIP release packaging requires Windows");
  execFileSync("powershell.exe", [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    "[System.IO.Compression.ZipFile]::CreateFromDirectory($env:NICECHUNK_RELEASE_STAGE, $env:NICECHUNK_RELEASE_ARCHIVE, [System.IO.Compression.CompressionLevel]::Optimal, $false)",
  ], {
    env: {
      ...process.env,
      NICECHUNK_RELEASE_ARCHIVE: archive,
      NICECHUNK_RELEASE_STAGE: stage,
    },
    stdio: "inherit",
  });
  const signature = (await readFile(archive)).subarray(0, 4).toString("hex");
  if (!["504b0304", "504b0506", "504b0708"].includes(signature)) {
    throw new Error("Windows release archive is not a valid ZIP container");
  }
} else {
  execFileSync("tar", ["-czf", archiveName, "-C", stageName, "."], { cwd: output, stdio: "inherit" });
}
const archiveHash = await sha256(archive);
await writeFile(`${archive}.sha256`, `${archiveHash}  ${basename(archive)}\n`);
await rm(stage, { recursive: true, force: true });

console.log(JSON.stringify({ archive, archiveName, platform, sha256: archiveHash }));

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
  for (const required of ["platform", "version", "binary", "output"]) {
    if (!result[required]) throw new Error(`Missing --${required}`);
  }
  return result;
}
