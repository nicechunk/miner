import { createServer } from "node:http";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium, firefox, webkit } from "playwright";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "web", "dist");
const requests = [];
const server = createServer(async (request, response) => {
  try {
    requests.push({ method: request.method, url: request.url });
    if (request.url === "/miner") {
      response.writeHead(308, { location: "/miner/" });
      response.end();
      return;
    }
    const pathname = new URL(request.url, "http://127.0.0.1").pathname;
    if (!pathname.startsWith("/miner/")) {
      response.writeHead(404, { "content-type": "text/plain" });
      response.end("not found");
      return;
    }
    const relative = pathname.slice("/miner/".length) || "index.html";
    const file = resolve(dist, normalize(relative));
    if (!file.startsWith(`${dist}/`) && file !== resolve(dist, "index.html")) throw new Error("path escape");
    const metadata = await stat(file);
    if (!metadata.isFile()) throw new Error("not a file");
    response.writeHead(200, {
      "content-type": contentType(extname(file)),
      "cache-control": file.endsWith("index.html") || file.endsWith("-manifest.json") ? "no-cache" : "public, max-age=31536000, immutable",
      "content-security-policy": "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
      "permissions-policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
    });
    response.end(await readFile(file));
  } catch {
    response.writeHead(404, { "content-type": "text/plain", "x-content-type-options": "nosniff" });
    response.end("not found");
  }
});

await new Promise((resolveReady) => server.listen(0, "127.0.0.1", resolveReady));
const address = server.address();
const origin = `http://127.0.0.1:${address.port}`;
try {
  let completed = 0;
  for (const target of browserTargets()) {
    let browser;
    try {
      browser = await target.type.launch({ headless: true, ...target.launchOptions });
    } catch (error) {
      if (target.required) throw error;
      console.log(`${target.label} smoke skipped: ${String(error.message || error).split("\n")[0]}`);
      continue;
    }
    try {
      await testBrowser(browser, target.label, origin, requests);
      completed += 1;
    } finally {
      await browser.close();
    }
  }
  assert(completed > 0, "No browser engine was available for smoke testing");
} finally {
  await new Promise((resolveClose) => server.close(resolveClose));
}

async function testBrowser(browser, label, origin, requests) {
  const requestStart = requests.length;
  const page = await browser.newPage();
  page.setDefaultTimeout(30_000);
  const errors = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (!request.url().startsWith(origin)) errors.push(`external request: ${request.url()}`);
    if (request.method() !== "GET") errors.push(`unexpected ${request.method()} request: ${request.url()}`);
  });

  const redirect = await page.request.get(`${origin}/miner`, { maxRedirects: 0 });
  assert(redirect.status() === 308, "/miner must redirect with 308");
  const missing = await page.request.get(`${origin}/miner/assets/does-not-exist.js`);
  assert(missing.status() === 404, "missing JS must return 404");
  assert(!(await missing.text()).includes("<!doctype html>"), "missing JS must not return HTML");

  await page.goto(`${origin}/miner/`, { waitUntil: "networkidle" });
  await page.locator('#minerWorldCanvas[data-scene-ready="true"]').waitFor({ state: "attached" });
  await page.waitForFunction(() => Number(document.getElementById("minerWorldCanvas")?.dataset.sceneTerrainChunks) >= 9);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-view") === "overview", `${label} scene should open on the world view`);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-renderer") === "chunk.js-webgl2", `${label} scene must use the Chunk.js WebGL2 renderer`);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-seed") === "nicechunk-mainnet-001", `${label} scene must use the mainnet world seed`);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-avatar") === "NCM:peasant_guy:v1", `${label} scene must use the game avatar`);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-cottage") === "NCM3:house-blueprint", `${label} scene must render the hardcoded game cottage`);
  assert((await page.locator("#minerWorldCanvas").getAttribute("data-scene-forge-item"))?.startsWith("forged-pickaxe:"), `${label} scene must render the game forged pickaxe`);
  assert(Number(await page.locator("#minerWorldCanvas").getAttribute("data-scene-terrain-thickness")) >= 20, `${label} terrain must have a visible rocky underside`);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-decorations") === "trees-grass-flowers", `${label} scene decorations are incomplete`);
  await page.waitForFunction(() => document.getElementById("engineBadge")?.classList.contains("ready"), null, { timeout: 30_000 });
  assert(await page.locator("#incumbentBytes").textContent() !== "—", "sample inspection should populate bytes");
  const englishHero = await page.locator(".hero-lede").textContent();
  const englishStatus = await page.locator("#statusBanner").textContent();
  const englishEngine = await page.locator("#engineBadge").textContent();
  for (const locale of ["es", "fr", "de", "ja", "ru", "ko", "zh-Hant", "zh-Hans"]) {
    await page.locator("#localeSelect").selectOption(locale);
    await page.waitForFunction((language) => document.documentElement.lang === language, locale);
    assert(await page.locator(".hero-lede").textContent() !== englishHero, `${label} ${locale} did not translate the Miner page`);
    assert(await page.locator("#statusBanner").textContent() !== englishStatus, `${label} ${locale} did not re-render dynamic Miner status text`);
    assert(await page.locator("#engineBadge").textContent() !== englishEngine, `${label} ${locale} did not re-render dynamic engine text`);
    assert(await page.evaluate(() => localStorage.getItem("nicechunk.language")) === locale, `${label} ${locale} selection was not persisted`);
  }
  await page.reload({ waitUntil: "networkidle" });
  await page.waitForFunction(() => document.documentElement.lang === "zh-Hans");
  assert(await page.locator("#localeSelect").inputValue() === "zh-Hans", `${label} persisted Miner locale was not restored`);
  await page.waitForFunction(() => document.getElementById("engineBadge")?.classList.contains("ready"), null, { timeout: 30_000 });
  await page.waitForFunction(() => document.getElementById("incumbentBytes")?.textContent !== "—", null, { timeout: 30_000 });
  await page.locator("#localeSelect").selectOption("en");
  await page.waitForFunction(() => document.documentElement.lang === "en");

  const profileViews = { terrain_delta: "terrain", building: "building", forged_item: "forged" };
  for (const profile of ["terrain_delta", "building", "forged_item"]) {
    await page.locator(`[data-scene-profile="${profile}"]`).click();
    await page.waitForFunction((value) => document.querySelector(`[data-profile="${value}"]`)?.getAttribute("aria-selected") === "true", profile);
    await page.waitForFunction((view) => document.getElementById("minerWorldCanvas")?.dataset.sceneView === view, profileViews[profile]);
    if (profile === "forged_item") {
      await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneActorRoles === "forged-item");
      assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-actor-count") === "1", `${label} forge view must isolate one forged item`);
    }
    try {
      await page.waitForFunction(() => document.getElementById("statusBanner")?.textContent.includes("Ready"), null, { timeout: 30_000 });
    } catch (error) {
      console.error("profile load diagnostics", {
        profile,
        language: await page.locator("html").getAttribute("lang"),
        status: await page.locator("#statusBanner").textContent(),
        engine: await page.locator("#engineBadge").textContent(),
        errors,
      });
      throw error;
    }
    await page.locator("#timeBudget").fill(profile === "terrain_delta" ? "10" : "2");
    await page.locator("#workerCount").fill("2");
    await page.locator("#populationInput").fill("8");
    await page.locator("#startButton").click();
    await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "running");
    if (profile === "terrain_delta") {
      await page.locator("#pauseButton").click();
      await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "paused");
      await page.waitForFunction(() => !document.getElementById("resumeButton")?.disabled);
      await page.locator("#resumeButton").click();
      await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "running");
      await page.waitForFunction(() => !document.getElementById("pauseButton")?.disabled);
    }
    assert(await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(() => resolveFrame(true)))), `${label} main thread did not reach an animation frame`);
    try {
      await page.waitForFunction(() => document.getElementById("exactStatus")?.textContent === "Exact Match", null, { timeout: 30_000 });
    } catch (error) {
      console.error("browser diagnostics", {
        profile,
        status: await page.locator("#statusBanner").textContent(),
        engine: await page.locator("#engineBadge").textContent(),
        errors,
      });
      throw error;
    }
    assert(await page.locator("#mismatchCount").textContent() === "0", `${profile} mismatch count should be zero`);
    assert(!(await page.locator("#downloadResult").isDisabled()), `${profile} result download should be enabled`);
    if (await page.locator("#stopButton").isEnabled()) await page.locator("#stopButton").click();
    await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "stopped");
    await page.waitForFunction(() => document.getElementById("workerStatus")?.textContent.startsWith("0 worker"), null, { timeout: 5_000 });
  }

  const mobile = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 1 });
  await mobile.addInitScript(() => localStorage.setItem("nicechunk.language", "zh-Hans"));
  mobile.on("console", (message) => {
    if (message.type() === "error") errors.push(`mobile: ${message.text()}`);
  });
  mobile.on("pageerror", (error) => errors.push(`mobile: ${error.message}`));
  mobile.on("request", (request) => {
    if (!request.url().startsWith(origin)) errors.push(`mobile external request: ${request.url()}`);
    if (request.method() !== "GET") errors.push(`mobile unexpected ${request.method()} request: ${request.url()}`);
  });
  await mobile.goto(`${origin}/miner/`, { waitUntil: "networkidle" });
  await mobile.waitForFunction(() => document.documentElement.lang === "zh-Hans");
  await mobile.locator('#minerWorldCanvas[data-scene-ready="true"]').waitFor({ state: "attached" });
  await mobile.waitForFunction(() => Number(document.getElementById("minerWorldCanvas")?.dataset.sceneTerrainChunks) >= 9);
  assert(await mobile.locator("#minerWorldCanvas").getAttribute("data-scene-renderer") === "chunk.js-webgl2", `${label} mobile scene must use the Chunk.js WebGL2 renderer`);
  assert(/\p{Script=Han}/u.test(await mobile.locator(".hero-lede").textContent()), `${label} mobile Miner locale was not applied`);
  assert(await mobile.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), `${label} mobile page overflows horizontally`);
  await mobile.locator("#headerMenuButton").click();
  assert(await mobile.locator("#primaryNav").getAttribute("data-open") === "true", `${label} mobile menu did not open`);
  await mobile.locator('[data-scene-view="building"][data-scene-profile="building"]').click();
  await mobile.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneView === "building");
  assert(await mobile.locator(".scene-dock").isVisible(), `${label} mobile camera dock is hidden`);
  await mobile.locator('[data-scene-view="forged"][data-scene-profile="forged_item"]').click();
  await mobile.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneView === "forged");
  await mobile.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneActorRoles === "forged-item");
  assert(await mobile.locator("#minerWorldCanvas").getAttribute("data-scene-actor-count") === "1", `${label} mobile forge view must isolate the pickaxe`);
  await mobile.close();

  const browserRequests = requests.slice(requestStart);
  const diagnostics = browserRequests.filter((request) => request.url !== "/miner/assets/does-not-exist.js");
  assert(errors.length === 0, `${label} browser errors: ${errors.join(" | ")} · requests: ${diagnostics.map((request) => `${request.method} ${request.url}`).join(", ")}`);
  assert(browserRequests.every((request) => request.method === "GET"), `${label} observed a non-GET request`);
  console.log(`${label} browser smoke passed with ${browserRequests.length} local GET requests and no uploads`);
}

function browserTargets() {
  const requested = String(process.env.POUW_BROWSER_TARGETS || "")
    .split(",")
    .map((value) => value.trim().toLowerCase())
    .filter(Boolean);
  const configured = {
    chromium: { label: "Chromium", type: chromium },
    firefox: { label: "Firefox", type: firefox },
    webkit: { label: "WebKit", type: webkit },
  };
  if (requested.length) {
    return requested.map((name) => {
      if (!configured[name]) throw new Error(`Unknown POUW_BROWSER_TARGETS entry ${name}`);
      return { ...configured[name], required: true };
    });
  }

  const targets = [];
  if (existsSync("/usr/bin/google-chrome")) {
    targets.push({ label: "Google Chrome", type: chromium, launchOptions: { executablePath: "/usr/bin/google-chrome" }, required: true });
  } else {
    targets.push({ ...configured.chromium, required: false });
  }
  targets.push({ ...configured.firefox, required: false }, { ...configured.webkit, required: false });
  if (existsSync("/usr/bin/microsoft-edge")) {
    targets.push({ label: "Microsoft Edge", type: chromium, launchOptions: { executablePath: "/usr/bin/microsoft-edge" }, required: false });
  }
  return targets;
}

function contentType(extension) {
  return {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".wasm": "application/wasm",
    ".png": "image/png",
    ".webp": "image/webp",
    ".ncbk": "application/octet-stream",
    ".ncm3": "text/plain; charset=utf-8",
    ".ncf1": "application/octet-stream",
  }[extension] || "application/octet-stream";
}

function assert(value, message) {
  if (!value) throw new Error(message);
}
