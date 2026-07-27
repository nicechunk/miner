import { execFileSync, spawn } from "node:child_process";
import { createServer } from "node:net";
import {
  appendFile,
  chmod,
  cp,
  lstat,
  mkdtemp,
  mkdir,
  readFile,
  readlink,
  realpath,
  rename,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "web", "dist");
const temporary = await mkdtemp(resolve(tmpdir(), "nicechunk-miner-nginx-"));
const webRoot = resolve(temporary, "web");
const minerRoot = resolve(webRoot, "miner");
const releases = resolve(minerRoot, "releases");
const nginxBinary = process.env.NGINX || "nginx";
let nginxProcess;

try {
  await chmod(temporary, 0o755);
  await mkdir(releases, { recursive: true });
  const releaseBuild = resolve(temporary, "release-build");
  const built = JSON.parse(execFileSync(process.execPath, [
    resolve(root, "scripts", "build-web-release.mjs"),
    "--dist", dist,
    "--output", releaseBuild,
  ], { encoding: "utf8" }).trim());
  const builtRelease = resolve(releaseBuild, built.releaseId);
  assert((await readFile(resolve(builtRelease, "index.html"), "utf8")).includes("<!doctype html>"), "Built release does not expose index.html at its root");
  assert(!(await exists(resolve(builtRelease, "site", "index.html"))), "Built release has an unexpected nested site directory");
  for (const release of ["release-one", "release-two"]) {
    await cp(builtRelease, resolve(releases, release), { recursive: true });
    await appendFile(resolve(releases, release, "index.html"), `\n<!-- ${release} -->\n`);
  }

  await atomicSwitch("release-one");
  await assertCurrent("release-one");
  await atomicSwitch("release-two");
  await assertCurrent("release-two");
  await atomicSwitch("release-one");
  await assertCurrent("release-one");

  const snippetSource = await readFile(resolve(root, "nginx", "miner-location.conf"), "utf8");
  assert(snippetSource.includes("return 308 /miner/;"), "Nginx snippet is missing the /miner redirect");
  assert(snippetSource.includes("application/wasm"), "Nginx snippet is missing the WASM MIME type");
  assert(snippetSource.includes("return 404;"), "Nginx snippet is missing an explicit fallback 404");

  if (!hasNginx()) {
    if (process.env.POUW_REQUIRE_NGINX === "1") throw new Error("nginx executable is required but unavailable");
    console.log("Offline release switch and rollback passed; nginx config/runtime smoke skipped because nginx is unavailable");
    process.exitCode = 0;
  } else {
    const port = await availablePort();
    const snippet = snippetSource.replaceAll("/web/nicechunk/miner", minerRoot);
    const config = resolve(temporary, "nginx.conf");
    await writeFile(config, `
worker_processes 1;
pid ${resolve(temporary, "nginx.pid")};
error_log stderr notice;
events { worker_connections 64; }
http {
    include /etc/nginx/mime.types;
    default_type application/octet-stream;
    access_log off;
    gzip on;
    gzip_min_length 256;
    gzip_types application/javascript application/json application/wasm text/css text/javascript text/plain;
    server {
        listen 127.0.0.1:${port};
        server_name localhost;
        location = /play/ { return 200 "game-ok"; }
        ${snippet}
        location / { return 200 "home-ok"; }
    }
}
`);
    execFileSync(nginxBinary, ["-t", "-c", config, "-p", temporary], { stdio: "inherit" });
    nginxProcess = spawn(nginxBinary, ["-c", config, "-p", temporary, "-g", "daemon off;"], {
      stdio: ["ignore", "inherit", "inherit"],
    });
    await waitForHttp(`http://127.0.0.1:${port}/miner/`);
    const origin = `http://127.0.0.1:${port}`;

    await smoke(origin, "release-one");
    await atomicSwitch("release-two");
    await smoke(origin, "release-two");
    await atomicSwitch("release-one");
    await smoke(origin, "release-one");
    assert((await readlink(resolve(minerRoot, "current"))) === "releases/release-one", "Rollback target is not release-one");
    console.log("Nginx config, atomic publish, rollback, MIME/cache/security, asset 404, gzip, and existing-route smoke passed");
  }
} finally {
  if (nginxProcess && nginxProcess.exitCode == null) {
    nginxProcess.kill("SIGTERM");
    await Promise.race([
      new Promise((resolveExit) => nginxProcess.once("exit", resolveExit)),
      new Promise((resolveTimeout) => setTimeout(resolveTimeout, 5_000)),
    ]);
  }
  await rm(temporary, { recursive: true, force: true });
}

async function smoke(origin, expectedRelease) {
  const redirect = await fetch(`${origin}/miner`, { redirect: "manual" });
  assert(redirect.status === 308, "/miner did not return 308");
  assert(redirect.headers.get("location") === `${origin}/miner/` || redirect.headers.get("location") === "/miner/", "/miner redirect target is wrong");

  const index = await fetch(`${origin}/miner/`);
  const html = await index.text();
  assert(
    index.status === 200 && html.includes(`<!-- ${expectedRelease} -->`),
    `Index did not come from ${expectedRelease}: status=${index.status}, tail=${JSON.stringify(html.slice(-120))}`,
  );
  assert(index.headers.get("cache-control")?.includes("no-cache"), "Index cache policy is not no-cache");
  assert(index.headers.get("content-type")?.startsWith("text/html"), "Index MIME type is wrong");
  assert(index.headers.get("x-content-type-options") === "nosniff", "Index is missing nosniff");
  assert(index.headers.get("content-security-policy")?.includes("wasm-unsafe-eval"), "Index CSP is missing WASM policy");
  assert(index.headers.get("permissions-policy")?.includes("camera=()"), "Index is missing Permissions-Policy");

  const manifest = JSON.parse(await readFile(resolve(dist, "asset-manifest.json"), "utf8"));
  const paths = [...Object.values(manifest.assets), ...Object.values(manifest.samples), ...Object.values(manifest.locales)]
    .map((path) => path.replace(/^\.\//u, "assets/"));
  for (const path of paths) {
    const response = await fetch(`${origin}/miner/${path}`);
    assert(response.status === 200, `${path} returned ${response.status}`);
    assert(response.headers.get("cache-control")?.includes("immutable"), `${path} is not immutable`);
    assert(response.headers.get("x-content-type-options") === "nosniff", `${path} is missing nosniff`);
    if (path.endsWith(".wasm")) assert(response.headers.get("content-type")?.startsWith("application/wasm"), "WASM MIME type is wrong");
  }

  const releaseManifest = await fetch(`${origin}/miner/release-manifest.json`);
  assert(releaseManifest.status === 200, "release-manifest.json is unavailable");
  assert(releaseManifest.headers.get("cache-control")?.includes("no-cache"), "Release manifest is not no-cache");

  for (const path of ["assets/missing.js", "assets/missing.wasm", "missing.json"]) {
    const response = await fetch(`${origin}/miner/${path}`);
    assert(response.status === 404, `${path} did not return 404`);
    assert(!(await response.text()).toLowerCase().includes("<!doctype html>"), `${path} returned HTML fallback`);
  }

  const compressed = await fetch(`${origin}/miner/${manifest.assets.app}`, {
    headers: { "accept-encoding": "gzip" },
  });
  assert(compressed.headers.get("content-encoding") === "gzip", "Hashed JavaScript was not gzip encoded");
  assert((await fetch(`${origin}/play/`)).status === 200, "Existing game route changed");
  assert(await (await fetch(`${origin}/play/`)).text() === "game-ok", "Existing game route response changed");
  assert(await (await fetch(`${origin}/`)).text() === "home-ok", "Existing website root response changed");
}

async function atomicSwitch(release) {
  const next = resolve(minerRoot, `.current-${process.pid}`);
  await rm(next, { force: true });
  await symlink(`releases/${release}`, next);
  await rename(next, resolve(minerRoot, "current"));
}

async function assertCurrent(release) {
  const actual = await realpath(resolve(minerRoot, "current"));
  const expected = await realpath(resolve(releases, release));
  assert(actual === expected, `Expected current=${release}, received ${actual}`);
}

function hasNginx() {
  try {
    execFileSync(nginxBinary, ["-v"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

async function availablePort() {
  const server = createServer();
  await new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise((resolveClose, reject) => server.close((error) => error ? reject(error) : resolveClose()));
  return port;
}

async function waitForHttp(url) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (nginxProcess?.exitCode != null) throw new Error(`nginx exited with ${nginxProcess.exitCode}`);
    try {
      const response = await fetch(url);
      if (response.status > 0) return;
    } catch {
      // Retry while nginx binds its local test socket.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  }
  throw new Error("nginx did not become ready");
}

function assert(value, message) {
  if (!value) throw new Error(message);
}

async function exists(path) {
  try {
    await lstat(path);
    return true;
  } catch {
    return false;
  }
}
