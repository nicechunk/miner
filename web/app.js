import { getLocale, initI18n, setLocale, t } from "__I18N_URL__";

const WORKER_URL = new URL("__WORKER_URL__", import.meta.url);
const SCENE_URL = new URL("__SCENE_URL__", import.meta.url);
const SITE_CONFIG_URL = new URL("__SITE_CONFIG_URL__", import.meta.url);
const RELEASE_MANIFEST_URL = new URL("../release-manifest.json", import.meta.url);
const ENGINE_REQUEST_TIMEOUT_MS = 120_000;
let worldScene = null;
let ncmPreviewScene = null;
let engineWorker = null;
let engineProbePromise = null;
let engineRequestId = 0;
const enginePendingRequests = new Map();
const checkpointPersistence = {
  inFlight: false,
  pending: null,
  timer: null,
};

const elements = Object.fromEntries([
  "localeSelect", "inputText", "loadTextButton", "analyzeButton", "decodeButton", "verifyButton",
  "workerCount", "seedInput", "populationInput", "startButton",
  "pauseButton", "resumeButton", "stopButton", "resetButton", "statusBanner",
  "engineBadge", "incumbentBytes", "candidateBytes", "savedBytes", "savedPercent",
  "attempts", "attemptRate", "elapsed", "workerStatus", "decodeUnits", "programBytes",
  "residualBytes", "overheadBytes", "targetRoot", "candidateRoot", "mismatchCount",
  "exactStatus", "curveCanvas", "downloadCandidate", "downloadResult", "downloadTask",
  "downloadCheckpoint", "checkpointInput", "downloadReport", "copyCommand", "releasePanel", "sourceNote",
  "inputFormat", "witnessStatus", "ncm4SeedBytes", "selectedFormat", "generation",
  "strategyName", "islandCount", "originalModelSummary", "candidateModelSummary",
  "diffOverlaySummary",
  "ncmPreviewFrame", "ncmPreviewCanvas", "ncmPreviewMessage", "ncmPreviewFormat",
  "ncmPreviewDimensions", "ncmPreviewVoxels", "ncmPreviewRoot",
].map((id) => [id, document.getElementById(id)]));

const state = {
  profile: "building",
  input: null,
  inputName: "",
  inspect: null,
  ncm4: null,
  ncm4Best: null,
  savedCheckpointBase64: null,
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
  inputRevision: 0,
};

initialize().catch((error) => fail(error));

async function initialize() {
  const logical = Math.max(1, Number(navigator.hardwareConcurrency || 2) - 1);
  const webKit = /AppleWebKit/iu.test(navigator.userAgent)
    && !/(?:Chrome|Chromium|Edg|OPR)\//iu.test(navigator.userAgent);
  elements.workerCount.value = String(webKit ? 1 : Math.min(8, logical));
  bindEvents();
  updateButtons();
  void initializeWorldScene();
  await initI18n();
  elements.localeSelect.value = getLocale();
  await loadSiteConfig();
  await loadReleaseManifest();
  await probeEngine();
  await loadPastedInput();
  drawCurve();
}

function bindEvents() {
  bindHeaderMenu();
  document.querySelectorAll("[data-profile]").forEach((button) => {
    button.addEventListener("click", () => {
      if (button.dataset.profile === state.profile) {
        dispatchSceneProfile();
        return;
      }
      beginInputLoad();
      state.phase = "idle";
      selectProfileWithoutLoading(button.dataset.profile);
      elements.inputText.value = "";
      dispatchScenePhase();
      setTranslatedStatus("idle", "status.ready", "status.loadFirst");
      updateButtons();
      render();
    });
  });
  document.querySelectorAll("[data-scene-profile]").forEach((button) => {
    button.addEventListener("click", () => {
      document.querySelector(`.profile-tabs [data-profile="${button.dataset.sceneProfile}"]`)?.click();
    });
  });
  elements.loadTextButton.addEventListener("click", () => loadPastedInput().catch(fail));
  elements.analyzeButton.addEventListener("click", () => analyzeNcm4(true).catch(fail));
  elements.decodeButton.addEventListener("click", () => decodeCurrent().catch(fail));
  elements.verifyButton.addEventListener("click", () => verifyCurrent().catch(fail));
  elements.startButton.addEventListener("click", () => startMining().catch(fail));
  elements.pauseButton.addEventListener("click", pauseMining);
  elements.resumeButton.addEventListener("click", resumeMining);
  elements.stopButton.addEventListener("click", () => stopWorkers("status.stoppedByUser"));
  elements.resetButton.addEventListener("click", () => reset().catch(fail));
  elements.localeSelect.addEventListener("change", () => changeLocale(elements.localeSelect.value).catch(fail));
  elements.downloadCandidate.addEventListener("click", () => downloadBase64(
    state.best?.candidateBase64,
    state.best?.format === "ncm4-pouw-v1" ? "candidate.nc4p" : "candidate.ncpow-vm",
    "application/octet-stream",
  ));
  elements.downloadCheckpoint.addEventListener("click", () => downloadBase64(state.best?.checkpointBase64 || state.savedCheckpointBase64, "miner-session.nc4s.chk", "application/octet-stream"));
  elements.checkpointInput.addEventListener("change", () => importCheckpoint().catch(fail));
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
    disposeEngineWorker();
    worldScene?.destroy();
    worldScene = null;
    ncmPreviewScene?.destroy();
    ncmPreviewScene = null;
  });
}

async function initializeWorldScene() {
  const canvas = document.getElementById("minerWorldCanvas");
  try {
    const { createMinerWorldScene, createNcmPreviewScene } = await import(SCENE_URL);
    if (canvas) {
      worldScene = createMinerWorldScene(canvas);
      dispatchScenePhase();
    }
    if (elements.ncmPreviewCanvas) {
      ncmPreviewScene = createNcmPreviewScene(elements.ncmPreviewCanvas, {
        onUnavailable: () => {
          elements.ncmPreviewMessage.removeAttribute("data-i18n");
          elements.ncmPreviewMessage.textContent = t("preview.webglUnavailable");
        },
      });
      ncmPreviewScene.setInspection(state.inspect);
    }
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
  if (state.engineVersion && !state.engineFailed) return state.engineVersion;
  if (!engineProbePromise) {
    engineProbePromise = requestEngine({ type: "version" })
      .then((response) => {
        state.engineVersion = response;
        state.engineFailed = false;
        renderEngineBadge();
        updateButtons();
        return response;
      })
      .catch((error) => {
        const engineError = asEngineFailure(error);
        disposeEngineWorker(engineError);
        throw engineError;
      })
      .finally(() => {
        engineProbePromise = null;
      });
  }
  return engineProbePromise;
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

async function loadPastedInput() {
  const value = elements.inputText.value.trim();
  if (!value) throw new Error(t("ncm4.pasteRequired"));
  const revision = beginInputLoad();
  const input = new TextEncoder().encode(value);
  const name = value.startsWith("NCM4P:") ? "pasted-input.nc4p" : "pasted-input.ncm3";
  await setInput(input, name, revision);
}

function beginInputLoad() {
  const revision = ++state.inputRevision;
  stopWorkers("status.inputChanged", false);
  state.input = null;
  state.inputName = "";
  state.inspect = null;
  state.ncm4 = null;
  state.ncm4Best = null;
  state.savedCheckpointBase64 = null;
  state.best = null;
  state.workerAttempts.clear();
  state.curve = [];
  state.elapsedBeforePause = 0;
  updateNcmPreview();
  enableDownloads(false);
  updateButtons();
  render();
  return revision;
}

async function setInput(input, name, revision) {
  if (revision !== state.inputRevision) return;
  state.input = input;
  state.inputName = name;
  state.inspect = null;
  state.ncm4 = null;
  state.ncm4Best = null;
  state.savedCheckpointBase64 = null;
  state.best = null;
  state.workerAttempts.clear();
  state.curve = [];
  state.elapsedBeforePause = 0;
  const detectedProfile = detectInputProfile(input, name);
  if (detectedProfile && detectedProfile !== state.profile) selectProfileWithoutLoading(detectedProfile);
  setTranslatedStatus("idle", "status.inspecting", "status.inspectingDetail");
  updateButtons();
  render();
  const profile = state.profile;
  try {
    await probeEngine();
    if (revision !== state.inputRevision) return;
    const response = await requestEngine(
      { type: "inspect", profile, input },
      [input.slice().buffer],
    );
    if (revision !== state.inputRevision) return;
    state.inspect = response;
    state.curve.push({ attempts: 0, bytes: response.incumbentBytes });
    updateNcmPreview();
    await analyzeNcm4(false, revision);
    state.savedCheckpointBase64 = await loadStoredCheckpoint(checkpointStorageKey(response));
    updateButtons();
    render();
    setTranslatedStatus("idle", "status.ready", "status.readyLoaded", {
      detailParams: { count: () => formatNumber(response.voxelCount) },
    });
  } catch (error) {
    if (revision === state.inputRevision) {
      state.inspect = null;
      updateNcmPreview();
      updateButtons();
      render();
    }
    throw error;
  }
}

async function analyzeNcm4(showStatus = true, revision = state.inputRevision) {
  if (!state.input || !state.inspect) return;
  if (showStatus) setTranslatedStatus("idle", "ncm4.analyzing", "ncm4.analyzingDetail");
  const input = state.input.slice();
  const analysis = await requestEngine(
    { type: "ncm4Analyze", profile: state.profile, input },
    [input.buffer],
  );
  if (revision !== state.inputRevision) return;
  state.ncm4 = analysis;
  state.ncm4Best = normalizeNcm4Analysis(analysis);
  state.best = state.ncm4Best;
  state.curve = [
    { attempts: 0, bytes: state.inspect.incumbentBytes },
    { attempts: 0, bytes: state.ncm4Best.candidateBytes },
  ];
  enableDownloads(true);
  drawCurve();
  render();
  if (showStatus) showBestStatus();
}

function normalizeNcm4Analysis(analysis) {
  const headerBytes = Number(analysis.fixedHeaderBytes || 0) + Number(analysis.profileHeaderBytes || 0);
  const sourceBytes = Number(analysis.sourceBytes || 0);
  const candidateBytes = Number(analysis.ncm4TotalBytes || 0);
  const savedBytes = Number.isFinite(Number(analysis.savedBytes))
    ? Number(analysis.savedBytes)
    : sourceBytes - candidateBytes;
  return {
    ...analysis,
    format: "ncm4-pouw-v1",
    accepted: Boolean(analysis.witnessExists),
    improved: Boolean(analysis.witnessExists),
    exact: Boolean(analysis.exact),
    mismatchCount: 0,
    incumbentBytes: sourceBytes,
    candidateBytes,
    savedBytes,
    savedBps: Number.isFinite(Number(analysis.savedBps))
      ? Number(analysis.savedBps)
      : sourceBytes === 0 ? 0 : Math.trunc(savedBytes * 10000 / sourceBytes),
    programBytes: Number(analysis.bodyBytes || 0),
    residualBytes: Number(analysis.residualBytes || 0),
    overheadBytes: headerBytes,
    targetSemanticRoot: analysis.semanticRoot,
    candidateSemanticRoot: analysis.candidateSemanticRoot || analysis.semanticRoot,
    candidateEncodingHash: analysis.encodingHash,
    generations: 0,
    generation: 0,
    attempts: 0,
    strategy: "deterministic-language-audit",
  };
}

function detectInputProfile(input, name) {
  if (input.length >= 6 && String.fromCharCode(...input.slice(0, 4)) === "NC4P") {
    return { 1: "terrain_delta", 2: "building", 3: "forged_item" }[input[5]] || null;
  }
  const prefix = new TextDecoder().decode(input.slice(0, Math.min(input.length, 24)));
  if (prefix.startsWith("NCM4P:")) return detectNcm4TextProfile(prefix);
  if (prefix.startsWith("NCM3:")) return "building";
  if (prefix.startsWith("NCBK")) return "terrain_delta";
  const lower = String(name || "").toLowerCase();
  if (lower.endsWith(".ncf1") || lower.endsWith(".ncf")) return "forged_item";
  if (lower.endsWith(".ncm3") || lower.endsWith(".ncm")) return "building";
  if (lower.endsWith(".ncbk")) return "terrain_delta";
  return null;
}

function detectNcm4TextProfile(prefix) {
  try {
    const encoded = prefix.slice("NCM4P:".length).replaceAll("-", "+").replaceAll("_", "/");
    const padded = encoded + "=".repeat((4 - encoded.length % 4) % 4);
    const header = atob(padded.slice(0, 12));
    if (header.slice(0, 4) !== "NC4P" || header.length < 6) return null;
    return { 1: "terrain_delta", 2: "building", 3: "forged_item" }[header.charCodeAt(5)] || null;
  } catch {
    return null;
  }
}

function selectProfileWithoutLoading(profile) {
  state.profile = profile;
  document.querySelectorAll("[data-profile]").forEach((item) => {
    item.setAttribute("aria-selected", String(item.dataset.profile === profile));
  });
  dispatchSceneProfile();
}

async function startMining() {
  if (state.engineFailed || !state.engineVersion) {
    updateButtons();
    setTranslatedStatus("failed", "errors.title", "errors.generic");
    return;
  }
  if (!state.input) {
    updateButtons();
    setTranslatedStatus("idle", "status.ready", "status.loadFirst");
    return;
  }
  if (!state.inspect) {
    updateButtons();
    setTranslatedStatus("idle", "status.inspecting", "status.inspectingDetail");
    return;
  }
  stopWorkers("status.restarting", false);
  state.best = state.profile === "building" ? state.ncm4Best : null;
  state.workerAttempts.clear();
  state.curve = [{ attempts: 0, bytes: state.inspect.incumbentBytes }];
  if (state.ncm4Best) state.curve.push({ attempts: 0, bytes: state.ncm4Best.candidateBytes });
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
      format: state.inspect.format,
      input: bytes,
      seed: (seed + index * 0x9e3779b9) >>> 0,
      population,
      workerCount,
      checkpointBase64: index === 0 ? state.savedCheckpointBase64 : null,
      sourceEncodingHash: state.inspect.encodingHash,
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
  scheduleCheckpointPersistence(
    checkpointStorageKey(state.inspect),
    state.savedCheckpointBase64,
    true,
  );
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
  scheduleCheckpointPersistence(
    checkpointStorageKey(state.inspect),
    state.savedCheckpointBase64,
    true,
  );
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
  elements.inputText.value = elements.inputText.defaultValue.trim();
  await loadPastedInput();
}

function tick() {
  if (state.phase !== "running") return;
  renderDynamicMetrics();
}

function handleWorkerMessage(workerId, message) {
  if (message.type === "error") {
    if (String(message.error).includes("checkpoint target or incumbent encoding")) {
      state.savedCheckpointBase64 = null;
    }
    fail(new Error(message.error));
    return;
  }
  if (message.type === "result") {
    const result = message.result;
    state.workerAttempts.set(workerId, Number(result.attempts || 0));
    if (result.checkpointBase64 && state.inspect?.semanticRoot) {
      state.savedCheckpointBase64 = result.checkpointBase64;
      scheduleCheckpointPersistence(checkpointStorageKey(state.inspect), result.checkpointBase64);
    }
    if (state.best && result.candidateEncodingHash === state.best.candidateEncodingHash) {
      state.best.checkpointBase64 = result.checkpointBase64;
      state.best.generation = result.generation ?? result.generations ?? state.best.generation;
      state.best.generations = state.best.generation;
      state.best.attempts = result.attempts ?? state.best.attempts;
    }
    if (result.exact && isBetter(result, state.best)) {
      state.best = result;
      state.curve.push({ attempts: totalAttempts(), bytes: result.candidateBytes });
      for (const [id, worker] of state.workers) {
        if (id !== workerId) {
          worker.postMessage({
            type: "verifiedBest",
            candidateBase64: result.candidateBase64,
            checkpointBase64: result.checkpointBase64,
          });
        }
      }
      enableDownloads(true);
      drawCurve();
      showBestStatus();
    }
    enableDownloads(Boolean(state.best));
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
  elements.inputFormat.textContent = inspect?.format || "—";
  elements.witnessStatus.textContent = state.ncm4
    ? t(state.ncm4.witnessExists ? "ncm4.witnessYes" : "ncm4.witnessNo")
    : "—";
  elements.witnessStatus.dataset.witness = String(Boolean(state.ncm4?.witnessExists));
  elements.ncm4SeedBytes.textContent = state.ncm4 ? formatBytes(state.ncm4.ncm4TotalBytes) : "—";
  elements.selectedFormat.textContent = best?.selectedFormat || state.ncm4?.selectedFormat || inspect?.format || "—";
  elements.generation.textContent = formatNumber(best?.generation ?? best?.generations ?? 0);
  elements.strategyName.textContent = strategyLabel(best?.strategy);
  elements.islandCount.textContent = formatNumber(state.workers.size);
  elements.originalModelSummary.textContent = semanticSummary(inspect?.semantics);
  elements.candidateModelSummary.textContent = semanticSummary(state.ncm4?.semantics || (best?.exact ? inspect?.semantics : null));
  elements.diffOverlaySummary.textContent = best
    ? t(best.exact ? "ncm4.diffExact" : "ncm4.diffMismatch", { count: best.mismatchCount ?? "—" })
    : "—";
  renderDynamicMetrics();
}

function updateNcmPreview() {
  const inspect = state.inspect;
  const semantics = inspect?.semantics;
  const building = semantics?.profile === "building" ? semantics.semantics : null;
  elements.ncmPreviewFormat.textContent = inspect?.format || "—";
  elements.ncmPreviewDimensions.textContent = Array.isArray(building?.size)
    ? building.size.join(" × ")
    : "—";
  elements.ncmPreviewVoxels.textContent = inspect ? formatNumber(inspect.voxelCount) : "—";
  elements.ncmPreviewRoot.textContent = inspect?.semanticRoot || "—";
  elements.ncmPreviewMessage.removeAttribute("data-i18n");
  if (elements.ncmPreviewCanvas.dataset.previewError) {
    elements.ncmPreviewMessage.textContent = t("preview.webglUnavailable");
  } else if (building) {
    elements.ncmPreviewMessage.textContent = t("preview.loading");
  } else if (inspect) {
    elements.ncmPreviewMessage.textContent = t("preview.buildingOnly");
  } else {
    elements.ncmPreviewMessage.textContent = t("preview.loading");
  }
  ncmPreviewScene?.setInspection(inspect);
}

function strategyLabel(strategy) {
  if (!strategy) return "—";
  const keys = {
    "deterministic-language-audit": "ncm4.strategyAudit",
    "beam-rewrite+typed-island-lns": "ncm4.strategyHybrid",
  };
  return keys[strategy] ? t(keys[strategy]) : strategy;
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
  } else if (state.inspect?.format === "ncm3-v1") {
    setTranslatedStatus("exact", "ncm4.ncm3Remains", "ncm4.ncm3RemainsDetail", {
      detailParams: { suffix: () => suffixKey ? t(suffixKey) : "" },
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
  const ready = Boolean(state.input && state.inspect && state.engineVersion && !state.engineFailed);
  elements.startButton.disabled = running || paused || !ready;
  elements.analyzeButton.disabled = running || paused || !ready;
  elements.decodeButton.disabled = running || paused || !ready;
  elements.verifyButton.disabled = running || paused || !ready || !state.best;
  elements.pauseButton.disabled = !running;
  elements.resumeButton.disabled = !paused;
  elements.stopButton.disabled = !running && !paused;
}

function enableDownloads(enabled) {
  elements.downloadCandidate.disabled = !enabled || !state.best?.candidateBase64;
  elements.downloadCheckpoint.disabled = !enabled || !(state.best?.checkpointBase64 || state.savedCheckpointBase64);
  elements.downloadResult.disabled = !enabled || !state.best?.resultBase64;
  elements.downloadTask.disabled = !enabled || !state.best?.taskBase64;
  elements.downloadReport.disabled = !enabled || !state.best;
  elements.copyCommand.disabled = !enabled || !state.best;
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

function ensureEngineWorker() {
  if (engineWorker) return engineWorker;
  try {
    engineWorker = new Worker(WORKER_URL, { type: "module", name: "nicechunk-pouw-control" });
  } catch (error) {
    throw asEngineFailure(error);
  }
  engineWorker.addEventListener("message", handleEngineMessage);
  engineWorker.addEventListener("error", (event) => {
    event.preventDefault();
    disposeEngineWorker(asEngineFailure(event.error || new Error(event.message || t("errors.engine"))));
  });
  engineWorker.addEventListener("messageerror", () => {
    disposeEngineWorker(asEngineFailure(new Error(t("errors.engine"))));
  });
  return engineWorker;
}

function requestEngine(message, transfer = []) {
  let worker;
  try {
    worker = ensureEngineWorker();
  } catch (error) {
    return Promise.reject(asEngineFailure(error));
  }
  const requestId = ++engineRequestId;
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      disposeEngineWorker(asEngineFailure(new Error(t("errors.workerTimeout"))));
    }, ENGINE_REQUEST_TIMEOUT_MS);
    enginePendingRequests.set(requestId, { resolve, reject, timeout });
    const payload = { ...message, requestId };
    if (payload.input instanceof Uint8Array && transfer.length) payload.input = new Uint8Array(transfer[0]);
    try {
      worker.postMessage(payload, transfer);
    } catch (error) {
      window.clearTimeout(timeout);
      enginePendingRequests.delete(requestId);
      reject(error);
    }
  });
}

function handleEngineMessage(event) {
  const message = event.data;
  const requestId = Number(message?.requestId);
  const pending = enginePendingRequests.get(requestId);
  if (!pending) return;
  window.clearTimeout(pending.timeout);
  enginePendingRequests.delete(requestId);
  if (message.type === "error") pending.reject(new Error(message.error));
  else if (message.type === "response") pending.resolve(message.result);
  else pending.reject(asEngineFailure(new Error(t("errors.engine"))));
}

function disposeEngineWorker(error = null) {
  const worker = engineWorker;
  engineWorker = null;
  worker?.terminate();
  for (const pending of enginePendingRequests.values()) {
    window.clearTimeout(pending.timeout);
    if (error) pending.reject(error);
  }
  enginePendingRequests.clear();
}

function asEngineFailure(error) {
  if (error?.engineFailure) return error;
  const wrapped = new Error(String(error?.message || error || t("errors.engine")));
  wrapped.engineFailure = true;
  return wrapped;
}

async function decodeCurrent() {
  if (!state.input || !state.inspect) return;
  if (state.inspect.format === "ncm4-pouw-v1") {
    const input = state.input.slice();
    const decoded = await requestEngine({ type: "ncm4Decode", input }, [input.buffer]);
    state.ncm4 ||= {
      ...decoded.stats,
      ncm4TotalBytes: decoded.stats.totalBytes,
      semanticRoot: decoded.semanticRoot,
      candidateSemanticRoot: decoded.semanticRoot,
      semantics: decoded.semantics,
      selectedFormat: "ncm4-pouw-v1",
      exact: true,
      witnessExists: false,
    };
  }
  render();
  setTranslatedStatus("exact", "ncm4.decoded", "ncm4.decodedDetail");
}

async function verifyCurrent() {
  if (!state.input || !state.best?.candidateBase64) return;
  if (state.best.format === "ncm4-pouw-v1") {
    const candidate = base64Bytes(state.best.candidateBase64);
    const report = await requestEngine({
      type: "ncm4Verify",
      profile: state.profile,
      source: state.input.slice(),
      candidate,
    });
    Object.assign(state.best, report);
  }
  render();
  showBestStatus();
}

async function importCheckpoint() {
  const file = elements.checkpointInput.files?.[0];
  if (!file) return;
  const bytes = new Uint8Array(await file.arrayBuffer());
  state.savedCheckpointBase64 = bytesBase64(bytes);
  enableDownloads(Boolean(state.best));
  setTranslatedStatus("idle", "ncm4.checkpointReady", "ncm4.checkpointReadyDetail");
}

function semanticSummary(value) {
  if (!value) return "—";
  const semantics = value.semantics || value;
  if (Array.isArray(semantics.voxels)) return t("ncm4.voxelSummary", { count: formatNumber(semantics.voxels.length) });
  if (Array.isArray(semantics.deleted)) return t("ncm4.voxelSummary", { count: formatNumber(semantics.deleted.length) });
  if (semantics.geometry?.components) {
    const count = semantics.geometry.components.reduce((sum, component) => sum + (component.solid?.length || 0), 0);
    return t("ncm4.voxelSummary", { count: formatNumber(count) });
  }
  if (semantics.geometry?.appearance?.quads) {
    return t("ncm4.quadSummary", { count: formatNumber(semantics.geometry.appearance.quads.length) });
  }
  return t("ncm4.semanticReady");
}

function openCheckpointDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("nicechunk-miner", 3);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains("ncm4-checkpoints-v2")) {
        request.result.createObjectStore("ncm4-checkpoints-v2", { keyPath: "key" });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function checkpointStorageKey(inspect) {
  return inspect?.semanticRoot && inspect?.encodingHash
    ? `${inspect.semanticRoot}:${inspect.encodingHash}`
    : null;
}

function scheduleCheckpointPersistence(key, checkpointBase64, immediate = false) {
  if (!key || !checkpointBase64 || !globalThis.indexedDB) return;
  checkpointPersistence.pending = { key, checkpointBase64 };
  if (checkpointPersistence.inFlight) return;
  if (checkpointPersistence.timer) {
    if (!immediate) return;
    window.clearTimeout(checkpointPersistence.timer);
  }
  checkpointPersistence.timer = window.setTimeout(
    drainCheckpointPersistence,
    immediate ? 0 : 750,
  );
}

async function drainCheckpointPersistence() {
  checkpointPersistence.timer = null;
  if (checkpointPersistence.inFlight || !checkpointPersistence.pending) return;
  const pending = checkpointPersistence.pending;
  checkpointPersistence.pending = null;
  checkpointPersistence.inFlight = true;
  try {
    await storeCheckpoint(pending.key, pending.checkpointBase64);
  } finally {
    checkpointPersistence.inFlight = false;
    if (checkpointPersistence.pending) {
      checkpointPersistence.timer = window.setTimeout(drainCheckpointPersistence, 750);
    }
  }
}

async function storeCheckpoint(key, checkpointBase64) {
  if (!key || !checkpointBase64 || !globalThis.indexedDB) return;
  try {
    const database = await openCheckpointDatabase();
    await new Promise((resolve, reject) => {
      const transaction = database.transaction("ncm4-checkpoints-v2", "readwrite");
      transaction.objectStore("ncm4-checkpoints-v2").put({
        key,
        checkpointBase64,
        savedAt: Date.now(),
      });
      transaction.oncomplete = resolve;
      transaction.onerror = () => reject(transaction.error);
    });
    database.close();
  } catch (error) {
    console.warn("Local checkpoint persistence is unavailable.", error);
  }
}

async function loadStoredCheckpoint(key) {
  if (!key || !globalThis.indexedDB) return null;
  try {
    const database = await openCheckpointDatabase();
    const record = await new Promise((resolve, reject) => {
      const request = database.transaction("ncm4-checkpoints-v2", "readonly")
        .objectStore("ncm4-checkpoints-v2")
        .get(key);
      request.onsuccess = () => resolve(request.result || null);
      request.onerror = () => reject(request.error);
    });
    database.close();
    return record?.checkpointBase64 || null;
  } catch {
    return null;
  }
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
  updateNcmPreview();
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
  const command = state.best?.format === "ncm4-pouw-v1"
    ? `nicechunk-miner ncm4 verify --source ${shellQuote(state.inputName || "source.ncm3")} --candidate candidate.nc4p`
    : "nicechunk-miner verify --task browser-task.ncpow --result browser-result.ncpow";
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

function bytesBase64(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
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
  if (error?.engineFailure) {
    state.engineFailed = true;
    state.engineVersion = null;
  }
  renderEngineBadge();
  setTranslatedStatus("failed", "errors.title", "errors.generic");
  stopWorkers("errors.stopped", false);
}
