import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium, firefox, webkit } from "playwright";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "web", "dist");
const sourceHtml = await readFile(resolve(root, "web", "index.html"), "utf8");
const defaultCottageNcm3 = sourceHtml.match(/<textarea id="inputText"[^>]*>(NCM3:[^<]+)<\/textarea>/u)?.[1];
assert(defaultCottageNcm3, "Default Hollow Cottage NCM3 input is missing");
const buildingInputs = Object.fromEntries(await Promise.all(
  ["normal-box", "boundary", "complex-cottage"].map(async (name) => [
    name,
    (await readFile(resolve(root, "test-vectors", "building", `${name}.ncm3`), "utf8")).trim(),
  ]),
));
const forgedInput = `NCF1.${Buffer.from(await readFile(
  resolve(root, "test-vectors", "forged_item", "complex-painted-cavity.ncf1"),
)).toString("base64url")}`;
const wasmDelayMs = Math.max(0, Number(process.env.POUW_WASM_DELAY_MS || 0));
const noWebGlOnly = process.env.POUW_NO_WEBGL_ONLY === "1";
const requestedBrowserNames = String(process.env.POUW_BROWSER_TARGETS || "")
  .split(",")
  .map((value) => value.trim().toLowerCase())
  .filter(Boolean);
if (requestedBrowserNames.length > 1 && process.env.POUW_BROWSER_CHILD !== "1") {
  for (const name of requestedBrowserNames) {
    execFileSync(process.execPath, [fileURLToPath(import.meta.url)], {
      env: {
        ...process.env,
        POUW_BROWSER_CHILD: "1",
        POUW_BROWSER_TARGETS: name,
      },
      stdio: "inherit",
    });
  }
  console.log(`Isolated browser smoke passed for ${requestedBrowserNames.join(", ")}`);
  process.exit(0);
}
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
    if (extname(file) === ".wasm" && wasmDelayMs > 0) {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, wasmDelayMs));
    }
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
      if (!noWebGlOnly) await testBrowser(browser, target.label, origin, requests);
      await testNoWebGlMining(browser, target.label, origin, requests);
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
  const initializationTimeout = Math.max(60_000, wasmDelayMs + 30_000);
  page.setDefaultTimeout(initializationTimeout);
  page.setDefaultNavigationTimeout(initializationTimeout);
  const observeWorkers = label !== "WebKit";
  if (observeWorkers) {
    await page.addInitScript(() => {
      const NativeWorker = window.Worker;
      window.__nicechunkWorkerNames = [];
      window.Worker = class NiceChunkObservedWorker extends NativeWorker {
        constructor(url, options) {
          super(url, options);
          window.__nicechunkWorkerNames.push(options?.name || "unnamed");
        }
      };
    });
  }
  const errors = [];
  const consoleDiagnostics = [];
  let ncm4CandidateBytes = null;
  page.on("console", (message) => {
    consoleDiagnostics.push(`${message.type()}: ${message.text()}`);
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
  try {
    await page.waitForFunction(() => (
      document.getElementById("minerWorldCanvas")?.dataset.sceneReady === "true"
      || document.documentElement.classList.contains("miner-scene-fallback")
    ));
  } catch (error) {
    console.error(`${label} scene initialization diagnostics`, {
      rootClass: await page.locator("html").getAttribute("class"),
      canvasData: await page.locator("#minerWorldCanvas").evaluate((canvas) => ({ ...canvas.dataset })),
      engine: await page.locator("#engineBadge").textContent(),
      status: await page.locator("#statusBanner").textContent(),
      diagnostics: consoleDiagnostics,
    });
    throw error;
  }
  const hasWebGlScene = await page.locator("#minerWorldCanvas").getAttribute("data-scene-ready") === "true";
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-view") === "overview", `${label} scene should open on the world view`);
  if (hasWebGlScene) {
    await page.waitForFunction(() => Number(document.getElementById("minerWorldCanvas")?.dataset.sceneTerrainChunks) >= 9);
    assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-renderer") === "chunk.js-webgl2", `${label} scene must use the Chunk.js WebGL2 renderer`);
    assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-seed") === "nicechunk-mainnet-001", `${label} scene must use the mainnet world seed`);
    assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-avatar") === "NCM:peasant_guy:v1", `${label} scene must use the game avatar`);
    assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-cottage") === "NCM3:house-blueprint", `${label} scene must render the hardcoded game cottage`);
    assert((await page.locator("#minerWorldCanvas").getAttribute("data-scene-forge-item"))?.startsWith("forged-pickaxe:"), `${label} scene must render the game forged pickaxe`);
    assert(Number(await page.locator("#minerWorldCanvas").getAttribute("data-scene-terrain-thickness")) >= 20, `${label} terrain must have a visible rocky underside`);
    assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-decorations") === "trees-grass-flowers", `${label} scene decorations are incomplete`);
  } else {
    assert((await page.locator("html").getAttribute("class"))?.includes("miner-scene-fallback"), `${label} did not expose the WebGL fallback state`);
    assert(await page.locator(".miner-world-fallback").isVisible(), `${label} static world fallback is hidden`);
  }
  await page.waitForFunction(() => document.getElementById("engineBadge")?.classList.contains("ready"), null, { timeout: initializationTimeout });
  await page.waitForFunction(() => document.getElementById("inputFormat")?.textContent === "ncm3-v1", null, { timeout: 30_000 });
  await page.locator("#ncmPreviewFrame").scrollIntoViewIfNeeded();
  try {
    await page.waitForFunction(() => {
      const canvas = document.getElementById("ncmPreviewCanvas");
      return canvas?.dataset.previewProfile === "building"
        && (canvas.dataset.previewReady === "true"
          || Boolean(canvas.dataset.previewFallback)
          || Boolean(canvas.dataset.previewError));
    }, null, { timeout: 30_000 });
  } catch (error) {
    console.error(`${label} default NCM preview diagnostics`, {
      canvas: await page.locator("#ncmPreviewCanvas").evaluate((canvas) => ({ ...canvas.dataset })),
      frame: await page.locator("#ncmPreviewFrame").getAttribute("data-preview-state"),
      message: await page.locator("#ncmPreviewMessage").textContent(),
      status: await page.locator("#statusBanner").textContent(),
      errors,
    });
    throw error;
  }
  assert(await page.locator("#inputText").inputValue() === defaultCottageNcm3, `${label} default paste input is not the current Hollow Cottage NCM3`);
  assert(await page.locator("#incumbentBytes").textContent() !== "—", "default pasted NCM inspection should populate bytes");
  assert(await page.locator("#ncmPreviewCanvas").getAttribute("data-preview-profile") === "building", `${label} preview must track the default NCM building`);
  assert(await page.locator("#ncmPreviewRoot").textContent() === await page.locator("#targetRoot").textContent(), `${label} preview root must come from the WASM inspection`);
  for (const removedId of ["sampleSelect", "loadSampleButton", "fileInput", "fileName", "timeBudget"]) {
    assert(await page.locator(`#${removedId}`).count() === 0, `${label} still exposes removed control #${removedId}`);
  }
  if (label === "WebKit") assert(await page.locator("#workerCount").inputValue() === "1", "WebKit must default to one mining worker");
  if (observeWorkers) {
    assert(await page.evaluate(() => window.__nicechunkWorkerNames.filter((name) => name === "nicechunk-pouw-control").length) === 1, `${label} must reuse one WASM control worker`);
  }
  const englishHero = await page.locator(".hero-lede").textContent();
  const englishStatus = await page.locator("#statusBanner").textContent();
  const englishEngine = await page.locator("#engineBadge").textContent();
  const localesToTest = label === "WebKit"
    ? ["zh-Hans"]
    : ["es", "fr", "de", "ja", "ru", "ko", "zh-Hant", "zh-Hans"];
  for (const locale of localesToTest) {
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
  await page.waitForFunction(() => document.getElementById("engineBadge")?.classList.contains("ready"), null, { timeout: initializationTimeout });
  await page.waitForFunction(() => document.getElementById("incumbentBytes")?.textContent !== "—", null, { timeout: 30_000 });
  await page.locator("#localeSelect").selectOption("en");
  await page.waitForFunction(() => document.documentElement.lang === "en");

  await page.locator('[data-scene-profile="terrain_delta"]').click();
  await page.waitForFunction(() => document.querySelector('[data-profile="terrain_delta"]')?.getAttribute("aria-selected") === "true");
  await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneView === "terrain");
  assert(await page.locator("#inputText").inputValue() === "", `${label} profile switch must not inject a built-in sample`);
  assert(await page.locator("#startButton").isDisabled(), `${label} Start must remain disabled until pasted input is inspected`);
  await page.locator("#resetButton").click();
  await page.waitForFunction(() => document.querySelector('[data-profile="building"]')?.getAttribute("aria-selected") === "true");
  await page.waitForFunction(() => document.getElementById("inputFormat")?.textContent === "ncm3-v1");
  assert(await page.locator("#inputText").inputValue() === defaultCottageNcm3, `${label} Reset did not restore the default cottage paste value`);

  const defaultPreview = await previewSnapshot(page);
  for (const variant of ["normal-box", "boundary"]) {
    await page.locator("#inputText").fill(buildingInputs[variant]);
    await page.locator("#loadTextButton").click();
    await page.waitForFunction((previousRoot) => {
      const canvas = document.getElementById("ncmPreviewCanvas");
      return canvas?.dataset.previewProfile === "building"
        && Boolean(canvas.dataset.previewSemanticRoot)
        && canvas.dataset.previewSemanticRoot !== previousRoot
        && document.getElementById("targetRoot")?.textContent === canvas.dataset.previewSemanticRoot
        && (canvas.dataset.previewReady === "true"
          || Boolean(canvas.dataset.previewFallback)
          || Boolean(canvas.dataset.previewError));
    }, defaultPreview.root, { timeout: 30_000 });
    const variantPreview = await previewSnapshot(page);
    assert(variantPreview.root !== defaultPreview.root, `${label} ${variant} reused the default cottage semantic root`);
    assert(variantPreview.root === await page.locator("#targetRoot").textContent(), `${label} ${variant} preview root differs from verification`);
    assert(JSON.stringify(variantPreview) !== JSON.stringify(defaultPreview), `${label} ${variant} NCM preview data did not change`);
  }

  await page.locator("#inputText").fill(forgedInput);
  await page.locator("#loadTextButton").click();
  await page.waitForFunction(() => {
    const canvas = document.getElementById("ncmPreviewCanvas");
    return document.getElementById("inputFormat")?.textContent === "ncf1-v15"
      && canvas?.dataset.previewProfile === "forged_item"
      && (canvas.dataset.previewAsset === "forged-item" || Boolean(canvas.dataset.previewFallback))
      && (canvas.dataset.previewReady === "true"
        || Boolean(canvas.dataset.previewFallback)
        || Boolean(canvas.dataset.previewError));
  }, null, { timeout: 30_000 });
  assert(await page.locator('[data-profile="forged_item"]').getAttribute("aria-selected") === "true", `${label} NCF1 paste did not select the forged-item mining profile`);
  assert(await page.locator("#ncmPreviewFormat").textContent() === "ncf1-v15", `${label} forged preview did not identify NCF1 v15`);
  assert(await page.locator("#ncmPreviewRoot").textContent() === await page.locator("#targetRoot").textContent(), `${label} forged preview semantic root differs from verification`);
  assert(Number((await page.locator("#ncmPreviewVoxels").textContent()).replaceAll(/\D/gu, "")) > 0, `${label} forged preview geometry count is empty`);
  if (hasWebGlScene) {
    assert(await page.locator("#ncmPreviewCanvas").getAttribute("data-preview-ready") === "true", `${label} forged item did not render through Chunk.js WebGL2`);
    assert(Number(await page.locator("#ncmPreviewCanvas").getAttribute("data-preview-mesh-triangles")) > 0, `${label} forged item mesh has no triangles`);
    const canvas = page.locator("#ncmPreviewCanvas");
    const box = await canvas.boundingBox();
    assert(box, `${label} forged preview canvas has no layout box`);
    const initialYaw = await canvas.getAttribute("data-preview-yaw");
    await page.mouse.move(box.x + box.width * 0.5, box.y + box.height * 0.55);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.68, box.y + box.height * 0.46, { steps: 5 });
    await page.mouse.up();
    assert(await canvas.getAttribute("data-preview-yaw") !== initialYaw, `${label} drag did not rotate the forged model camera`);
    const initialZoom = await canvas.getAttribute("data-preview-zoom");
    await page.mouse.wheel(0, -180);
    assert(await canvas.getAttribute("data-preview-zoom") !== initialZoom, `${label} wheel did not zoom the forged model camera`);
    const initialPan = await canvas.getAttribute("data-preview-pan");
    await page.keyboard.down("Shift");
    await page.mouse.move(box.x + box.width * 0.52, box.y + box.height * 0.56);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.6, box.y + box.height * 0.63, { steps: 4 });
    await page.mouse.up();
    await page.keyboard.up("Shift");
    assert(await canvas.getAttribute("data-preview-pan") !== initialPan, `${label} shift-drag did not pan the forged model camera`);
    await page.locator("#previewResetButton").click();
    assert(await canvas.getAttribute("data-preview-yaw") === "0.73000", `${label} preview reset did not restore yaw`);
    assert(await canvas.getAttribute("data-preview-zoom") === "1.00000", `${label} preview reset did not restore zoom`);
  }
  await page.locator("#workerCount").fill("1");
  await page.locator("#populationInput").fill("4");
  await page.locator("#startButton").click();
  await page.waitForFunction(() => (
    document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "running"
    && Number(document.getElementById("attempts")?.textContent.replaceAll(/\D/gu, "") || 0) > 0
    && document.getElementById("exactStatus")?.textContent === "Exact Match"
  ), null, { timeout: 30_000 });
  await page.locator("#stopButton").click();

  await page.locator("#inputText").fill(buildingInputs["complex-cottage"]);
  await page.locator("#loadTextButton").click();
  await page.waitForFunction(() => document.getElementById("statusBanner")?.textContent.includes("Ready"), null, { timeout: 30_000 });
  await page.waitForFunction(() => {
    const canvas = document.getElementById("ncmPreviewCanvas");
    return canvas?.dataset.previewProfile === "building"
      && (canvas.dataset.previewReady === "true"
        || Boolean(canvas.dataset.previewFallback)
        || Boolean(canvas.dataset.previewError));
  }, null, { timeout: 30_000 });
  assert(await page.locator("#ncmPreviewFormat").textContent() === "ncm3-v1", `${label} building preview must identify the real NCM3 source`);
  assert(/^\d+ × \d+ × \d+$/u.test(await page.locator("#ncmPreviewDimensions").textContent()), `${label} building preview dimensions are missing`);
  assert(Number((await page.locator("#ncmPreviewVoxels").textContent()).replaceAll(",", "")) > 0, `${label} building preview voxel count is missing`);
  assert(await page.locator("#ncmPreviewRoot").textContent() === await page.locator("#targetRoot").textContent(), `${label} building preview semantic root differs from verification`);
  if (hasWebGlScene) {
    assert(await page.locator("#ncmPreviewCanvas").getAttribute("data-preview-ready") === "true", `${label} NCM preview did not render through Chunk.js WebGL2`);
    assert(Number(await page.locator("#ncmPreviewCanvas").getAttribute("data-preview-chunks")) > 0, `${label} NCM preview has no rendered chunks`);
  } else {
    assert(["unavailable", "error"].includes(await page.locator("#ncmPreviewFrame").getAttribute("data-preview-state")), `${label} NCM preview did not expose its WebGL fallback`);
    assert(await page.locator(".ncm-preview-fallback").isVisible(), `${label} NCM preview fallback is hidden`);
  }
  if (observeWorkers) {
    assert(await page.evaluate(() => window.__nicechunkWorkerNames.filter((name) => name === "nicechunk-pouw-control").length) === 1, `${label} created duplicate WASM control workers`);
  }

  await page.locator("#workerCount").fill(label === "WebKit" ? "1" : "2");
  await page.locator("#populationInput").fill("8");
  await page.locator("#startButton").click();
  await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "running");
  assert(await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(() => resolveFrame(true)))), `${label} main thread did not reach an animation frame`);
  try {
    await page.waitForFunction(() => document.getElementById("exactStatus")?.textContent === "Exact Match", null, { timeout: 30_000 });
  } catch (error) {
    console.error("browser diagnostics", {
      profile: "building",
      status: await page.locator("#statusBanner").textContent(),
      engine: await page.locator("#engineBadge").textContent(),
      errors,
    });
    throw error;
  }
  assert(await page.locator("#mismatchCount").textContent() === "0", "building mismatch count should be zero");
  await page.waitForFunction(() => Number(document.getElementById("generation")?.textContent || 0) >= 2);
  const generationWhileRunning = Number(await page.locator("#generation").textContent());
  await page.waitForTimeout(750);
  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-phase") === "running", `${label} search stopped without a user action`);
  await page.waitForFunction((previous) => Number(document.getElementById("generation")?.textContent || 0) > previous, generationWhileRunning);
  const generationBeforePause = Number(await page.locator("#generation").textContent());
  await page.locator("#pauseButton").click();
  await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "paused");
  await page.locator("#resumeButton").click();
  await page.waitForFunction((previous) => Number(document.getElementById("generation")?.textContent || 0) > previous, generationBeforePause);
  assert(await page.locator("#inputFormat").textContent() === "ncm3-v1", `${label} building source format was not detected`);
  assert(await page.locator("#selectedFormat").textContent() === "ncm4-pouw-v1", `${label} shorter NCM4 witness was not selected`);
  assert(await page.locator("#candidateBytes").textContent() === "57 B", `${label} deterministic NCM4 witness byte count changed`);
  assert(!(await page.locator("#downloadCandidate").isDisabled()), `${label} NCM4 candidate download should be enabled`);
  assert(!(await page.locator("#downloadCheckpoint").isDisabled()), `${label} NCM4 checkpoint download should be enabled`);
  assert(!(await page.locator("#downloadReport").isDisabled()), `${label} NCM4 JSON report should be enabled`);
  assert(await page.locator("#downloadResult").isDisabled(), `${label} NCM4 must not expose a nonexistent NCPV result`);
  assert(await page.locator("#downloadTask").isDisabled(), `${label} NCM4 must not expose a nonexistent NCPV task`);
  const downloadPromise = page.waitForEvent("download");
  await page.locator("#downloadCandidate").click();
  const download = await downloadPromise;
  ncm4CandidateBytes = await readFile(await download.path());
  assert(ncm4CandidateBytes.length === 57, `${label} downloaded NCM4 candidate has the wrong byte length`);
  await page.evaluate(() => document.getElementById("stopButton")?.click());
  await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "stopped");
  await page.waitForFunction(() => document.getElementById("workerStatus")?.textContent.startsWith("0 worker"), null, { timeout: 5_000 });
  const semanticRoot = await page.locator("#targetRoot").textContent();
  await page.waitForFunction((root) => new Promise((resolveCheckpoint) => {
    const request = indexedDB.open("nicechunk-miner", 3);
    request.onerror = () => resolveCheckpoint(false);
    request.onsuccess = () => {
      const database = request.result;
      const lookup = database.transaction("ncm4-checkpoints-v2", "readonly")
        .objectStore("ncm4-checkpoints-v2")
        .getAll();
      lookup.onerror = () => resolveCheckpoint(false);
      lookup.onsuccess = () => {
        database.close();
        resolveCheckpoint(lookup.result.some((record) => (
          record.key?.startsWith(`${root}:`) && record.checkpointBase64
        )));
      };
    };
  }), semanticRoot, { timeout: 5_000 });

  assert(ncm4CandidateBytes, `${label} did not produce an NCM4 candidate for import testing`);
  {
    await page.locator('[data-scene-profile="terrain_delta"]').click();
    await page.waitForFunction(() => document.querySelector('[data-profile="terrain_delta"]')?.getAttribute("aria-selected") === "true");
    const textEncoding = `NCM4P:${Buffer.from(ncm4CandidateBytes).toString("base64url")}`;
    await page.locator("#inputText").fill(textEncoding);
    await page.locator("#loadTextButton").click();
    await page.waitForFunction(() => document.querySelector('[data-profile="building"]')?.getAttribute("aria-selected") === "true");
    await page.waitForFunction(() => document.getElementById("inputFormat")?.textContent === "ncm4-pouw-v1");
    await page.waitForFunction(() => {
      const canvas = document.getElementById("ncmPreviewCanvas");
      return canvas?.dataset.previewFormat === "ncm4-pouw-v1"
        && (canvas.dataset.previewReady === "true"
          || Boolean(canvas.dataset.previewFallback)
          || Boolean(canvas.dataset.previewError));
    });
    assert(await page.locator("#ncmPreviewRoot").textContent() === await page.locator("#targetRoot").textContent(), `${label} imported NCM4 preview root differs from verification`);
    assert(await page.locator("#candidateBytes").textContent() === "57 B", `${label} imported NCM4P text changed size`);
    await page.locator("#workerCount").fill("1");
    await page.locator("#populationInput").fill("4");
    await page.locator("#startButton").click();
    await page.waitForFunction(() => Number(document.getElementById("generation")?.textContent || 0) > 0);
    assert(await page.locator("#exactStatus").textContent() === "Exact Match", `${label} NCM4 source search lost exactness`);
    await page.evaluate(() => {
      const button = document.getElementById("stopButton");
      if (button && !button.disabled) button.click();
    });
    await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "stopped");
    await page.waitForFunction(() => document.getElementById("workerStatus")?.textContent.startsWith("0 worker"));
  }

  const mobile = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 1 });
  mobile.setDefaultTimeout(initializationTimeout);
  mobile.setDefaultNavigationTimeout(initializationTimeout);
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
  await mobile.waitForFunction(() => (
    document.getElementById("minerWorldCanvas")?.dataset.sceneReady === "true"
    || document.documentElement.classList.contains("miner-scene-fallback")
  ));
  const mobileHasWebGlScene = await mobile.locator("#minerWorldCanvas").getAttribute("data-scene-ready") === "true";
  if (mobileHasWebGlScene) {
    await mobile.waitForFunction(() => Number(document.getElementById("minerWorldCanvas")?.dataset.sceneTerrainChunks) >= 9);
    assert(await mobile.locator("#minerWorldCanvas").getAttribute("data-scene-renderer") === "chunk.js-webgl2", `${label} mobile scene must use the Chunk.js WebGL2 renderer`);
  } else {
    assert(await mobile.locator(".miner-world-fallback").isVisible(), `${label} mobile static fallback is hidden`);
  }
  assert(/\p{Script=Han}/u.test(await mobile.locator(".hero-lede").textContent()), `${label} mobile Miner locale was not applied`);
  assert(await mobile.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), `${label} mobile page overflows horizontally`);
  await mobile.locator("#headerMenuButton").click();
  assert(await mobile.locator("#primaryNav").getAttribute("data-open") === "true", `${label} mobile menu did not open`);
  await mobile.locator("#headerMenuButton").click();
  assert(await mobile.locator("#primaryNav").getAttribute("data-open") === "false", `${label} mobile menu did not close`);
  await mobile.locator('[data-scene-view="building"][data-scene-profile="building"]').click();
  await mobile.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneView === "building");
  await mobile.locator("#ncmPreviewFrame").scrollIntoViewIfNeeded();
  await mobile.waitForFunction(() => {
    const canvas = document.getElementById("ncmPreviewCanvas");
    return canvas?.dataset.previewProfile === "building"
      && (canvas.dataset.previewReady === "true"
        || Boolean(canvas.dataset.previewFallback)
        || Boolean(canvas.dataset.previewError));
  }, null, { timeout: 30_000 });
  if (mobileHasWebGlScene) {
    assert(await mobile.locator("#ncmPreviewCanvas").getAttribute("data-preview-ready") === "true", `${label} mobile NCM preview did not render`);
  } else {
    assert(["unavailable", "error"].includes(await mobile.locator("#ncmPreviewFrame").getAttribute("data-preview-state")), `${label} mobile NCM fallback state is missing`);
  }
  assert(await mobile.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), `${label} mobile NCM preview overflows horizontally`);
  assert(await mobile.locator(".scene-dock").isVisible(), `${label} mobile camera dock is hidden`);
  await mobile.locator('[data-scene-view="forged"][data-scene-profile="forged_item"]').click();
  await mobile.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneView === "forged");
  if (mobileHasWebGlScene) {
    await mobile.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.sceneActorRoles === "forged-item");
    assert(await mobile.locator("#minerWorldCanvas").getAttribute("data-scene-actor-count") === "1", `${label} mobile forge view must isolate the pickaxe`);
  }
  await mobile.close();

  const browserRequests = requests.slice(requestStart);
  const diagnostics = browserRequests.filter((request) => request.url !== "/miner/assets/does-not-exist.js");
  assert(errors.length === 0, `${label} browser errors: ${errors.join(" | ")} · requests: ${diagnostics.map((request) => `${request.method} ${request.url}`).join(", ")}`);
  assert(browserRequests.every((request) => request.method === "GET"), `${label} observed a non-GET request`);
  assert(browserRequests.every((request) => !request.url.includes("/samples/")), `${label} requested a removed built-in sample asset`);
  console.log(`${label} browser smoke passed with ${browserRequests.length} local GET requests and no uploads`);
}

async function previewSnapshot(page) {
  return page.evaluate(() => {
    const canvas = document.getElementById("ncmPreviewCanvas");
    return {
      root: canvas?.dataset.previewSemanticRoot || "",
      dimensions: canvas?.dataset.previewDimensions || "",
      voxels: Number(canvas?.dataset.previewVoxelCount || 0),
      chunks: Number(canvas?.dataset.previewChunks || 0),
    };
  });
}

async function testNoWebGlMining(browser, label, origin, requests) {
  const requestStart = requests.length;
  const page = await browser.newPage();
  const initializationTimeout = Math.max(60_000, wasmDelayMs + 30_000);
  page.setDefaultTimeout(initializationTimeout);
  page.setDefaultNavigationTimeout(initializationTimeout);
  await page.addInitScript(() => {
    const nativeGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function getContext(type, ...options) {
      if (String(type).toLowerCase() === "webgl2") return null;
      return nativeGetContext.call(this, type, ...options);
    };
  });

  const errors = [];
  const warnings = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
    if (message.type() === "warning") warnings.push(message.text());
  });
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (!request.url().startsWith(origin)) errors.push(`external request: ${request.url()}`);
    if (request.method() !== "GET") errors.push(`unexpected ${request.method()} request: ${request.url()}`);
  });

  await page.goto(`${origin}/miner/`, { waitUntil: "networkidle" });
  await page.waitForFunction(() => (
    document.documentElement.classList.contains("miner-scene-fallback")
    && document.getElementById("minerWorldCanvas")?.dataset.sceneCapability === "webgl2-unavailable"
  ));
  await page.waitForFunction(() => document.getElementById("engineBadge")?.classList.contains("ready"));
  await page.waitForFunction(() => document.getElementById("inputFormat")?.textContent === "ncm3-v1");
  await page.waitForFunction(() => {
    const canvas = document.getElementById("ncmPreviewCanvas");
    return canvas?.dataset.previewProfile === "building"
      && Boolean(canvas.dataset.previewSemanticRoot)
      && Number(canvas.dataset.previewVoxelCount) > 0;
  });

  assert(await page.locator("#minerWorldCanvas").getAttribute("data-scene-renderer") === "static-fallback", `${label} no-WebGL world did not use the static renderer`);
  assert(await page.locator(".miner-world-fallback").isVisible(), `${label} no-WebGL world fallback is hidden`);
  assert(await page.locator("#ncmPreviewFrame").getAttribute("data-preview-state") === "unavailable", `${label} no-WebGL NCM preview was treated as an application error`);
  assert(await page.locator("#ncmPreviewCanvas").getAttribute("data-preview-renderer") === "static-fallback", `${label} no-WebGL NCM preview renderer is incorrect`);
  assert(await page.locator(".ncm-preview-fallback").isVisible(), `${label} no-WebGL canonical summary is hidden`);
  assert(await page.locator("#ncmPreviewRoot").textContent() === await page.locator("#targetRoot").textContent(), `${label} no-WebGL semantic root differs from the WASM result`);
  assert(Number((await page.locator("#ncmPreviewVoxels").textContent()).replaceAll(/\D/gu, "")) > 0, `${label} no-WebGL voxel summary is empty`);

  await page.locator("#workerCount").fill("1");
  await page.locator("#populationInput").fill("4");
  await page.locator("#startButton").click();
  await page.waitForFunction(() => (
    document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "running"
    && Number(document.getElementById("generation")?.textContent.replaceAll(/\D/gu, "") || 0) > 0
    && Number(document.getElementById("attempts")?.textContent.replaceAll(/\D/gu, "") || 0) > 0
  ));
  assert(await page.locator("#mismatchCount").textContent() === "0", `${label} no-WebGL mining lost exactness`);

  await page.locator("#pauseButton").click();
  await page.waitForFunction(() => document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "paused");
  const generationBeforeResume = Number((await page.locator("#generation").textContent()).replaceAll(/\D/gu, ""));
  await page.locator("#resumeButton").click();
  await page.waitForFunction((previous) => (
    document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "running"
    && Number(document.getElementById("generation")?.textContent.replaceAll(/\D/gu, "") || 0) > previous
  ), generationBeforeResume);
  await page.locator("#stopButton").click();
  await page.waitForFunction(() => (
    document.getElementById("minerWorldCanvas")?.dataset.scenePhase === "stopped"
    && document.getElementById("islandCount")?.textContent === "0"
  ));

  const browserRequests = requests.slice(requestStart);
  assert(!browserRequests.some((request) => request.url.includes("miner-world-scene")), `${label} loaded the WebGL scene module without WebGL2 support`);
  assert(errors.length === 0, `${label} no-WebGL browser errors: ${errors.join(" | ")}`);
  assert(!warnings.some((warning) => /webgl\s*2|scene initialization/iu.test(warning)), `${label} no-WebGL capability fallback emitted an exception warning: ${warnings.join(" | ")}`);
  assert(browserRequests.every((request) => request.method === "GET"), `${label} no-WebGL run observed a non-GET request`);
  await page.close();
  console.log(`${label} no-WebGL CPU mining smoke passed without loading the 3D module`);
}

function browserTargets() {
  const configured = {
    chromium: { label: "Chromium", type: chromium },
    firefox: { label: "Firefox", type: firefox },
    webkit: { label: "WebKit", type: webkit },
    chrome: {
      label: "Google Chrome",
      type: chromium,
      launchOptions: { executablePath: "/usr/bin/google-chrome" },
    },
    edge: {
      label: "Microsoft Edge",
      type: chromium,
      launchOptions: { executablePath: "/usr/bin/microsoft-edge" },
    },
  };
  if (requestedBrowserNames.length) {
    return requestedBrowserNames.map((name) => {
      if (!configured[name]) throw new Error(`Unknown POUW_BROWSER_TARGETS entry ${name}`);
      if (configured[name].launchOptions?.executablePath
        && !existsSync(configured[name].launchOptions.executablePath)) {
        throw new Error(`${configured[name].label} executable is unavailable`);
      }
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
