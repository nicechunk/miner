import { lstat, readFile, readdir } from "node:fs/promises";
import { basename, dirname, extname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const minerRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const requestedRoot = process.argv.includes("--root")
  ? resolve(process.argv[process.argv.indexOf("--root") + 1] || "")
  : minerRoot;
const findings = [];

const skippedDirectories = new Set([
  ".git", ".toolchains", ".cache", "target", "node_modules", "artifacts",
]);
const binaryExtensions = new Set([
  ".wasm", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".zip", ".gz",
  ".ncbk", ".ncf", ".ncf1", ".woff", ".woff2",
]);
const forbiddenPathPatterns = [
  { label: "environment file", pattern: /(^|\/)\.env(?:\.|$)/iu, allow: /\.env\.example$/iu },
  { label: "private-key filename", pattern: /(?:^|\/)(?:id_rsa|id_ed25519|[^/]+\.(?:pem|p12|pfx|key))$/iu },
  { label: "wallet/keypair filename", pattern: /(?:wallet|keypair|service[-_]?account|credentials).*\.json$/iu },
  { label: "private operations directory", pattern: /(?:^|\/)(?:\.auth|\.deploy|\.ssh|\.gh-config)(?:\/|$)/u },
];
const contentPatterns = [
  { label: "private key block", pattern: /-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----/u },
  { label: "GitHub token", pattern: /\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})\b/u },
  { label: "AWS access key", pattern: /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/u },
  { label: "Google API key", pattern: /\bAIza[0-9A-Za-z_-]{35}\b/u },
  { label: "Slack token", pattern: /\bxox[baprs]-[0-9A-Za-z-]{20,}\b/u },
  { label: "Stripe secret", pattern: /\bsk_(?:live|test)_[0-9A-Za-z]{20,}\b/u },
  { label: "credential in URL", pattern: /\b(?:https?|ssh):\/\/[^\s/@:]+:[^\s/@]+@/iu },
  { label: "hard-coded bearer", pattern: /Authorization\s*:\s*Bearer\s+(?!\$|%|\{|<)[A-Za-z0-9._~-]{20,}/iu },
  {
    label: "hard-coded secret assignment",
    pattern: /(?:api[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|password|private[_-]?key)\s*[:=]\s*["'](?!\$|%|\{|<|REDACTED|example)[^"'\s]{12,}["']/iu,
  },
];

await walk(requestedRoot);
if (findings.length) {
  console.error(`Secret audit failed with ${findings.length} finding(s):`);
  for (const finding of findings) console.error(`- ${finding.label}: ${finding.path}`);
  process.exitCode = 1;
} else {
  console.log(`Secret audit passed for ${relative(process.cwd(), requestedRoot) || "."}; no credential-like files or values found`);
}

async function walk(path) {
  const metadata = await lstat(path);
  const normalized = relative(requestedRoot, path).split(sep).join("/");
  if (metadata.isSymbolicLink()) {
    findings.push({ label: "symlink in audited release tree", path: normalized });
    return;
  }
  if (metadata.isDirectory()) {
    if (path !== requestedRoot && skippedDirectories.has(basename(path))) return;
    for (const entry of await readdir(path)) await walk(resolve(path, entry));
    return;
  }
  if (!metadata.isFile()) {
    findings.push({ label: "special file", path: normalized });
    return;
  }

  for (const rule of forbiddenPathPatterns) {
    if (rule.pattern.test(normalized) && !rule.allow?.test(normalized)) {
      findings.push({ label: rule.label, path: normalized });
    }
  }
  if (binaryExtensions.has(extname(path).toLowerCase()) || metadata.size > 8 * 1024 * 1024) return;
  const bytes = await readFile(path);
  if (bytes.subarray(0, 8_192).includes(0)) return;
  const text = bytes.toString("utf8");
  for (const rule of contentPatterns) {
    if (rule.pattern.test(text)) findings.push({ label: rule.label, path: normalized });
  }
}
