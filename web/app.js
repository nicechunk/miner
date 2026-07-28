import { getLocale, initI18n, setLocale, t } from "__I18N_URL__";

const WORKER_URL = new URL("__WORKER_URL__", import.meta.url);
const SCENE_URL = new URL("__SCENE_URL__", import.meta.url);
const SITE_CONFIG_URL = new URL("__SITE_CONFIG_URL__", import.meta.url);
const RELEASE_MANIFEST_URL = new URL("../release-manifest.json", import.meta.url);
const SAMPLE_URLS = __SAMPLE_URLS__;
let worldScene = null;

const elements = Object.fromEntries([
  "localeSelect", "sampleSelect", "loadSampleButton", "fileInput", "fileName",
  "workerCount", "seedInput", "timeBudget", "populationInput", "startButton",
  "pauseButton", "resumeButton", "stopButton", "resetButton", "statusBanner",
  "engineBadge", "incumbentBytes", "candidateBytes", "savedBytes", "savedPercent",
  "attempts", "attemptRate", "elapsed", "workerStatus", "decodeUnits", "programBytes",
  "residualBytes", "overheadBytes", "targetRoot", "candidateRoot", "mismatchCount",
  "exactStatus", "curveCanvas", "downloadCandidate", "downloadResult", "downloadTask",
  "downloadReport", "copyCommand", "releasePanel", "sourceNote",
].map((id) => [id, document.getElementById(id)]));

const state = {
  profile: "terrain_delta",
  input: null,
  inputName: "",
  inspect: null,
  workers: new Map(),
  workerAttempts: new Map(),
  best: null,
  curve: [],
  phase: "idle",
  runStartedAt: 0,
  elapsedBeforePause: 0,
  timer: null,
  autoPaused: false,
  releaseManifest: null,
  releaseLoadError: null,
  engineVersion: null,
  engineFailed: false,
  statusView: null,
};

initialize().catch((error) => fail(error));

async function initialize() {
  const logical = Math.max(1, Number(navigator.hardwareConcurrency || 2) - 1);
  elements.workerCount.value = String(Math.min(8, logical));
  bindEvents();
  void initializeWorldScene();
  await initI18n();
  elements.localeSelect.value = getLocale();
  await loadSiteConfig();
  await loadReleaseManifest();
  await probeEngine();
  await loadSample();
  drawCurve();
}

function bindEvents() {
  bindHeaderMenu();
  document.querySelectorAll("[data-profile]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (state.phase === "running" || state.phase === "paused") stopWorkers("status.profileChanged");
      state.profile = button.dataset.profile;
      document.querySelectorAll("[data-profile]").forEach((item) => {
        item.setAttribute("aria-selected", String(item === button));
      });
      dispatchSceneProfile();
      await loadSample();
    });
  });
  document.querySelectorAll("[data-scene-profile]").forEach((button) => {
    button.addEventListener("click", () => {
      document.querySelector(`.profile-tabs [data-profile="${button.dataset.sceneProfile}"]`)?.click();
    });
  });
  elements.loadSampleButton.addEventListener("click", () => loadSample().catch(fail));
  elements.sampleSelect.addEventListener("change", () => loadSample().catch(fail));
  elements.fileInput.addEventListener("change", () => loadLocalFile().catch(fail));
  elements.startButton.addEventListener("click", () => startMining().catch(fail));
  elements.pauseButton.addEventListener("click", pauseMining);
  elements.resumeButton.addEventListener("click", resumeMining);
  elements.stopButton.addEventListener("click", () => stopWorkers("status.stoppedByUser"));
  elements.resetButton.addEventListener("click", () => reset().catch(fail));
  elements.localeSelect.addEventListener("change", () => changeLocale(elements.localeSelect.value).catch(fail));
  elements.downloadCandidate.addEventListener("click", () => downloadBase64(state.best?.candidateBase64, "candidate.ncpow-vm", "application/octet-stream"));
  elements.downloadResult.addEventListener("click", () => downloadBase64(state.best?.resultBase64, "browser-result.ncpow", "application/octet-stream"));
  elements.downloadTask.addEventListener("click", () => downloadBase64(state.best?.taskBase64, "browser-task.ncpow", "application/octet-stream"));
  elements.downloadReport.addEventListener("click", downloadReport);
  elements.copyCommand.addEventListener("click", copyVerifyCommand);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden && state.phase === "running") {
      state.autoPaused = true;
      pauseMining();
      setTranslatedStatus("paused", "status.paused", "status.hiddenPaused");
    }
  });
  window.addEventListener("pagehide", () => {
    stopWorkers("status.pageClosed", false);
    worldScene?.destroy();
    worldScene = null;
  });
}

async function initializeWorldScene() {
  const canvas = document.getElementById("minerWorldCanvas");
  if (!canvas) return;
  try {
    const { createMinerWorldScene } = await import(SCENE_URL);
    worldScene = createMinerWorldScene(canvas);
    dispatchScenePhase();
  } catch (error) {
    document.documentElement.classList.add("miner-scene-fallback");
    console.warn("NiceChunk 3D world scene is unavailable; using the static fallback.", error);
  }
}

function dispatchSceneProfile() {
  window.dispatchEvent(new CustomEvent("nicechunk:minerprofile", {
    detail: { profile: state.profile },
  }));
}

function dispatchScenePhase() {
  window.dispatchEvent(new CustomEvent("nicechunk:minerphase", {
    detail: { phase: state.phase },
  }));
}

function bindHeaderMenu() {
  const button = document.getElementById("headerMenuButton");
  const navigation = document.getElementById("primaryNav");
  if (!button || !navigation) return;

  const setOpen = (open) => {
    navigation.dataset.open = String(open);
    button.setAttribute("aria-expanded", String(open));
  };

  button.addEventListener("click", () => setOpen(button.getAttribute("aria-expanded") !== "true"));
  navigation.addEventListener("click", (event) => {
    if (event.target.closest?.("a[href]")) setOpen(false);
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") setOpen(false);
  });
  window.addEventListener("resize", () => {
    if (window.innerWidth > 1180) setOpen(false);
  }, { passive: true });
}

async function probeEngine() {
  const response = await oneShotWorker({ type: "version" });
  state.engineVersion = response;
  state.engineFailed = false;
  renderEngineBadge();
}

function renderEngineBadge() {
  elements.engineBadge.removeAttribute("data-i18n");
  if (state.engineFailed) {
    elements.engineBadge.textContent = t("errors.engine");
    elements.engineBadge.className = "engine-badge error";
  } else if (state.engineVersion) {
    elements.engineBadge.textContent = t("runtime.engineReady", {
      software: state.engineVersion.softwareVersion,
      protocol: state.engineVersion.protocolVersion,
      vm: state.engineVersion.vmVersion,
    });
    elements.engineBadge.className = "engine-badge ready";
  }
}

async function loadSample() {
  const key = `${state.profile}:${elements.sampleSelect.value}`;
  const url = SAMPLE_URLS[key];
  if (!url) throw new Error(t("errors.sampleMissing", { key }));
  const response = await fetch(new URL(url, import.meta.url), { cache: "no-cache" });
  if (!response.ok) throw new Error(t("errors.sampleHttp", { status: response.status }));
  const input = new Uint8Array(await response.arrayBuffer());
  await setInput(input, url.split("/").at(-1));
}

async function loadLocalFile() {
  const file = elements.fileInput.files?.[0];
  if (!file) return;
  const input = new Uint8Array(await file.arrayBuffer());
  await setInput(input, file.name);
}

async function setInput(input, name) {
  stopWorkers("status.inputChanged", false);
  state.input = input;
  state.inputName = name;
  state.best = null;
  state.workerAttempts.clear();
  state.curve = [];
  state.elapsedBeforePause = 0;
  elements.fileName.removeAttribute("data-i18n");
  elements.fileName.textContent = name;
  setTranslatedStatus("idle", "status.inspecting", "status.inspectingDetail");
  const response = await oneShotWorker({ type: "inspect", profile: state.profile, input }, [input.slice().buffer]);
  state.inspect = response;
  state.curve.push({ attempts: 0, bytes: response.incumbentBytes });
  updateButtons();
  render();
  setTranslatedStatus("idle", "status.ready", "status.readyLoaded", {
    detailParams: { count: () => formatNumber(response.voxelCount) },
  });
}

async function startMining() {
  if (!state.input) throw new Error(t("status.loadFirst"));
  stopWorkers("status.restarting", false);
  state.best = null;
  state.workerAttempts.clear();
  state.curve = [{ attempts: 0, bytes: state.inspect.incumbentBytes }];
  state.elapsedBeforePause = 0;
  state.runStartedAt = performance.now();
  state.phase = "running";
  dispatchScenePhase();
  state.autoPaused = false;
  const workerCount = clampInteger(elements.workerCount.value, 1, 16);
  const seed = clampInteger(elements.seedInput.value, 0, 0xffffffff);
  const population = clampInteger(elements.populationInput.value, 4, 256);
  for (let index = 0; index < workerCount; index += 1) {
    const worker = new Worker(WORKER_URL, { type: "module", name: `nicechunk-pouw-${index}` });
    worker.addEventListener("message", (event) => handleWorkerMessage(index, event.data));
    worker.addEventListener("error", (event) => {
      console.error(event.error || event.message);
      fail(new Error(t("errors.workerFailed", { index })));
    });
    state.workers.set(index, worker);
    const bytes = state.input.slice();
    worker.postMessage({
      type: "start",
      workerId: index,
      profile: state.profile,
      input: bytes,
      seed: (seed + index * 0x9e3779b9) >>> 0,
      population,
    }, [bytes.buffer]);
  }
  updateButtons();
  setTranslatedStatus("running", "status.searching", "status.searchingDetail");
  state.timer = window.setInterval(tick, 100);
  render();
}

function pauseMining() {
  if (state.phase !== "running") return;
  state.elapsedBeforePause += performance.now() - state.runStartedAt;
  state.phase = "paused";
  dispatchScenePhase();
  for (const worker of state.workers.values()) worker.postMessage({ type: "pause" });
  updateButtons();
  setTranslatedStatus("paused", "status.paused", "status.pausedDetail");
  render();
}

function resumeMining() {
  if (state.phase !== "paused") return;
  state.phase = "running";
  dispatchScenePhase();
  state.runStartedAt = performance.now();
  state.autoPaused = false;
  for (const worker of state.workers.values()) worker.postMessage({ type: "resume" });
  updateButtons();
  setTranslatedStatus("running", "status.searching", "status.resumedDetail");
}

function stopWorkers(messageKey = "status.stopped", showStatus = true) {
  if (state.timer) window.clearInterval(state.timer);
  state.timer = null;
  if (state.phase === "running") state.elapsedBeforePause += performance.now() - state.runStartedAt;
  for (const worker of state.workers.values()) {
    worker.postMessage({ type: "stop" });
    worker.terminate();
  }
  state.workers.clear();
  if (state.phase !== "idle") state.phase = "stopped";
  dispatchScenePhase();
  updateButtons();
  render();
  if (showStatus) {
    if (state.best?.exact) showBestStatus(messageKey);
    else setTranslatedStatus("idle", "status.stopped", messageKey);
  }
}

async function reset() {
  stopWorkers("status.reset", false);
  state.phase = "idle";
  dispatchScenePhase();
  elements.fileInput.value = "";
  await loadSample();
}

function tick() {
  if (state.phase !== "running") return;
  const budgetMs = clampInteger(elements.timeBudget.value, 1, 300) * 1000;
  if (elapsedMs() >= budgetMs) {
    stopWorkers("status.timeCompleted");
    return;
  }
  renderDynamicMetrics();
}

function handleWorkerMessage(workerId, message) {
  if (message.type === "error") {
    fail(new Error(message.error));
    return;
  }
  if (message.type === "result") {
    const result = message.result;
    state.workerAttempts.set(workerId, Number(result.attempts || 0));
    if (result.exact && isBetter(result, state.best)) {
      state.best = result;
      state.curve.push({ attempts: totalAttempts(), bytes: result.candidateBytes });
      for (const [id, worker] of state.workers) {
        if (id !== workerId) {
          worker.postMessage({ type: "verifiedBest", candidateBase64: result.candidateBase64 });
        }
      }
      enableDownloads(true);
      drawCurve();
      showBestStatus();
    }
    render();
  }
}

function isBetter(candidate, current) {
  if (!current) return true;
  if (Boolean(candidate.exact) !== Boolean(current.exact)) return Boolean(candidate.exact);
  if (candidate.candidateBytes !== current.candidateBytes) return candidate.candidateBytes < current.candidateBytes;
  if (candidate.decodeUnits !== current.decodeUnits) return candidate.decodeUnits < current.decodeUnits;
  return candidate.candidateEncodingHash < current.candidateEncodingHash;
}

function render() {
  const inspect = state.inspect;
  const best = state.best;
  elements.incumbentBytes.textContent = inspect ? formatBytes(inspect.incumbentBytes) : "—";
  elements.candidateBytes.textContent = best ? formatBytes(best.candidateBytes) : "—";
  elements.savedBytes.textContent = best ? formatSignedBytes(best.savedBytes) : "—";
  elements.savedPercent.textContent = best ? `${(best.savedBps / 100).toFixed(2)}%` : "—";
  elements.decodeUnits.textContent = best ? formatNumber(best.decodeUnits) : "—";
  elements.programBytes.textContent = best?.programBytes ?? 0;
  elements.residualBytes.textContent = best?.residualBytes ?? 0;
  elements.overheadBytes.textContent = best?.overheadBytes ?? 0;
  elements.targetRoot.textContent = best?.targetSemanticRoot || inspect?.semanticRoot || "—";
  elements.candidateRoot.textContent = best?.candidateSemanticRoot || "—";
  elements.mismatchCount.textContent = best ? formatNumber(best.mismatchCount) : "—";
  elements.exactStatus.removeAttribute("data-i18n");
  elements.exactStatus.textContent = best ? (best.exact ? t("metrics.exactMatch") : t("metrics.failed")) : t("metrics.notRun");
  renderDynamicMetrics();
}

function renderDynamicMetrics() {
  const attempts = totalAttempts();
  const elapsed = elapsedMs();
  elements.attempts.textContent = formatNumber(attempts);
  elements.attemptRate.textContent = t("runtime.perSecond", { rate: elapsed > 0 ? (attempts * 1000 / elapsed).toFixed(2) : "0.00" });
  elements.elapsed.textContent = t("runtime.seconds", { seconds: (elapsed / 1000).toFixed(1) });
  elements.workerStatus.textContent = t(state.workers.size === 1 ? "runtime.workerOne" : "runtime.workerMany", { count: state.workers.size });
}

function showBestStatus(suffixKey = "") {
  if (!state.best) return;
  if (!state.best.exact) {
    setTranslatedStatus("failed", "status.verificationFailed", "status.mismatchDetail", {
      detailParams: { count: () => formatNumber(state.best.mismatchCount) },
    });
  } else if (state.best.improved) {
    setTranslatedStatus("exact", "status.exactSmaller", "status.savedDetail", {
      detailParams: {
        saved: () => formatSignedBytes(state.best.savedBytes),
        suffix: () => suffixKey ? t(suffixKey) : "",
      },
    });
  } else {
    setTranslatedStatus("exact", "status.exactNoImprovement", "status.noImprovementDetail", {
      detailParams: { suffix: () => suffixKey ? t(suffixKey) : "" },
    });
  }
}

function setTranslatedStatus(kind, titleKey, detailKey, { titleParams = {}, detailParams = {} } = {}) {
  state.statusView = { kind, titleKey, detailKey, titleParams, detailParams };
  renderStatus();
}

function renderStatus() {
  if (!state.statusView) return;
  const { kind, titleKey, detailKey, titleParams, detailParams } = state.statusView;
  elements.statusBanner.className = `status-banner ${kind}`;
  const heading = elements.statusBanner.querySelector("strong");
  const body = elements.statusBanner.querySelector("span:last-child");
  heading.removeAttribute("data-i18n");
  body.removeAttribute("data-i18n");
  heading.textContent = t(titleKey, resolveDynamicParameters(titleParams));
  body.textContent = t(detailKey, resolveDynamicParameters(detailParams)).trim();
}

function resolveDynamicParameters(parameters) {
  return Object.fromEntries(Object.entries(parameters).map(([key, value]) => [
    key,
    typeof value === "function" ? value() : value,
  ]));
}

function updateButtons() {
  const running = state.phase === "running";
  const paused = state.phase === "paused";
  elements.startButton.disabled = running || paused || !state.input;
  elements.pauseButton.disabled = !running;
  elements.resumeButton.disabled = !paused;
  elements.stopButton.disabled = !running && !paused;
}

function enableDownloads(enabled) {
  for (const element of [elements.downloadCandidate, elements.downloadResult, elements.downloadTask, elements.downloadReport, elements.copyCommand]) {
    element.disabled = !enabled;
  }
}

function drawCurve() {
  const canvas = elements.curveCanvas;
  const context = canvas.getContext("2d");
  if (!context) return;
  const width = canvas.width;
  const height = canvas.height;
  context.clearRect(0, 0, width, height);
  context.fillStyle = "#0c0e10";
  context.fillRect(0, 0, width, height);
  context.strokeStyle = "#283137";
  context.lineWidth = 1;
  for (let row = 1; row < 4; row += 1) {
    const y = row * height / 4;
    context.beginPath(); context.moveTo(0, y); context.lineTo(width, y); context.stroke();
  }
  if (state.curve.length < 2) return;
  const maxAttempts = Math.max(1, ...state.curve.map((item) => item.attempts));
  const values = state.curve.map((item) => item.bytes);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = Math.max(1, max - min);
  context.strokeStyle = "#c8f72b";
  context.lineWidth = 4;
  context.lineJoin = "round";
  context.beginPath();
  state.curve.forEach((item, index) => {
    const x = 14 + (width - 28) * item.attempts / maxAttempts;
    const y = 14 + (height - 28) * (item.bytes - min) / range;
    if (index === 0) context.moveTo(x, y); else context.lineTo(x, y);
  });
  context.stroke();
}

async function oneShotWorker(message, transfer = []) {
  const worker = new Worker(WORKER_URL, { type: "module", name: "nicechunk-pouw-probe" });
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      worker.terminate();
      reject(new Error(t("errors.workerTimeout")));
    }, 30_000);
    worker.addEventListener("message", (event) => {
      if (event.data.type === "error") {
        window.clearTimeout(timeout); worker.terminate(); reject(new Error(event.data.error));
      } else if (event.data.type === "response") {
        window.clearTimeout(timeout); worker.terminate(); resolve(event.data.result);
      }
    });
    worker.addEventListener("error", (event) => {
      console.error(event.error || event.message);
      window.clearTimeout(timeout); worker.terminate(); reject(new Error(t("errors.workerFailed", { index: "probe" })));
    });
    const payload = { ...message };
    if (payload.input instanceof Uint8Array && transfer.length) payload.input = new Uint8Array(transfer[0]);
    worker.postMessage(payload, transfer);
  });
}

async function loadSiteConfig() {
  const response = await fetch(SITE_CONFIG_URL, { cache: "no-cache" });
  if (!response.ok) return;
  const config = await response.json();
  document.querySelectorAll("[data-link]").forEach((link) => {
    const value = config[link.dataset.link];
    if (typeof value === "string" && /^https:\/\/github\.com\//u.test(value)) link.href = value;
  });
}

async function loadReleaseManifest() {
  try {
    const response = await fetch(RELEASE_MANIFEST_URL, { cache: "no-cache" });
    if (!response.ok) throw new Error(t("errors.manifestHttp", { status: response.status }));
    const manifest = await response.json();
    state.releaseManifest = manifest;
    state.releaseLoadError = null;
    renderReleaseManifest(manifest);
  } catch (error) {
    state.releaseManifest = null;
    state.releaseLoadError = String(error.message || error);
    renderReleaseError();
  }
}

function renderReleaseError() {
  elements.releasePanel.className = "release-panel unavailable";
  elements.releasePanel.replaceChildren(
    createTextElement("h3", t("release.unavailableTitle")),
    createTextElement("p", t("release.loadError", { error: state.releaseLoadError || t("errors.generic") })),
  );
}

function renderReleaseManifest(manifest) {
  elements.releasePanel.replaceChildren();
  if (!manifest.available || !Array.isArray(manifest.artifacts) || !manifest.artifacts.length) {
    elements.releasePanel.className = "release-panel unavailable";
    elements.releasePanel.append(
      createTextElement("h3", t("release.notPublished")),
      createTextElement("p", t("release.notPublishedDetail")),
      createTextElement("small", t("release.versions", { protocol: manifest.protocolVersion ?? 1, vm: manifest.vmVersion ?? 1 })),
    );
    return;
  }
  elements.releasePanel.className = "release-panel";
  elements.releasePanel.append(createTextElement("h3", t("release.version", { version: manifest.softwareVersion })));
  for (const artifact of manifest.artifacts) {
    if (!artifact.downloadUrl || !artifact.sha256) continue;
    const entry = document.createElement("div");
    entry.className = "artifact";
    const link = document.createElement("a");
    link.className = "button compact secondary";
    link.href = artifact.downloadUrl;
    link.rel = "noreferrer";
    link.textContent = artifact.platform;
    entry.append(link, createTextElement("code", artifact.sha256));
    elements.releasePanel.append(entry);
  }
}

async function changeLocale(locale) {
  const resolvedLocale = await setLocale(locale);
  elements.localeSelect.value = resolvedLocale;
  renderEngineBadge();
  renderStatus();
  if (state.releaseManifest) renderReleaseManifest(state.releaseManifest);
  else if (state.releaseLoadError) renderReleaseError();
  render();
}

function downloadBase64(value, name, type) {
  if (!value) return;
  const bytes = base64Bytes(value);
  downloadBlob(new Blob([bytes], { type }), name);
}

function downloadReport() {
  if (!state.best) return;
  const report = { ...state.best };
  delete report.candidateBase64;
  delete report.resultBase64;
  delete report.taskBase64;
  delete report.checkpointBase64;
  downloadBlob(new Blob([`${JSON.stringify(report, null, 2)}\n`], { type: "application/json" }), "verification-report.json");
}

async function copyVerifyCommand() {
  const command = "nicechunk-miner verify --task browser-task.ncpow --result browser-result.ncpow";
  try {
    await navigator.clipboard.writeText(command);
  } catch {
    const area = document.createElement("textarea");
    area.value = command;
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
  elements.copyCommand.textContent = t("actions.copied");
  window.setTimeout(() => { elements.copyCommand.textContent = t("actions.copy"); }, 1500);
}

function downloadBlob(blob, name) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}

function base64Bytes(value) {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

function elapsedMs() {
  return state.elapsedBeforePause + (state.phase === "running" ? performance.now() - state.runStartedAt : 0);
}

function totalAttempts() {
  return [...state.workerAttempts.values()].reduce((sum, value) => sum + value, 0);
}

function formatBytes(value) {
  return `${formatNumber(value)} B`;
}

function formatSignedBytes(value) {
  const numeric = Number(value);
  return `${numeric > 0 ? "+" : ""}${formatNumber(numeric)} B`;
}

function formatNumber(value) {
  return Number(value).toLocaleString(getLocale());
}

function clampInteger(value, minimum, maximum) {
  const numeric = Math.trunc(Number(value));
  return Math.max(minimum, Math.min(maximum, Number.isFinite(numeric) ? numeric : minimum));
}

function createTextElement(tag, text) {
  const element = document.createElement(tag);
  element.textContent = text;
  return element;
}

function fail(error) {
  console.error(error);
  state.engineFailed = true;
  renderEngineBadge();
  setTranslatedStatus("failed", "errors.title", "errors.generic");
  stopWorkers("errors.stopped", false);
}
