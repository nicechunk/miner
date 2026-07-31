import init, {
  BrowserNcm4Session,
  baseline_json,
  decode_ncm4_json,
  detect_input_json,
  inspect_json,
  migrate_checkpoint_elite,
  mine_slice_json,
  ncm4_analyze_json,
  resume_slice_json,
  verify_local_json,
  verify_ncm4_json,
  version_json,
} from "__WASM_GLUE_URL__";

let initialized;
let workerId = 0;
let profile;
let input;
let seed;
let population;
let checkpoint;
let paused = false;
let stopped = false;
let running = false;
let externalBest;
let ncm4Session;
let ncm4Mode = false;
let externalCheckpoint;
let ncm4NeedsTargetValidation = false;
let sourceEncodingHash;

self.addEventListener("message", (event) => {
  handle(event.data).catch((error) => {
    self.postMessage({
      type: "error",
      requestId: event.data?.requestId,
      workerId,
      error: String(error?.message || error),
    });
  });
});

async function handle(message) {
  await ensureInitialized();
  if (message.type === "version") {
    respond(JSON.parse(version_json()), message.requestId);
  } else if (message.type === "detect") {
    respond(JSON.parse(detect_input_json(message.input)), message.requestId);
  } else if (message.type === "inspect") {
    respond(JSON.parse(inspect_json(message.profile, message.input)), message.requestId);
  } else if (message.type === "baseline") {
    respond(JSON.parse(baseline_json(message.profile, message.input)), message.requestId);
  } else if (message.type === "ncm4Analyze") {
    respond(JSON.parse(ncm4_analyze_json(message.profile, message.input)), message.requestId);
  } else if (message.type === "ncm4Decode") {
    respond(JSON.parse(decode_ncm4_json(message.input)), message.requestId);
  } else if (message.type === "ncm4Verify") {
    respond(JSON.parse(verify_ncm4_json(message.profile, message.source, message.candidate)), message.requestId);
  } else if (message.type === "start") {
    workerId = message.workerId;
    profile = message.profile;
    input = new Uint8Array(message.input);
    seed = Number(message.seed) >>> 0;
    population = Number(message.population);
    checkpoint = null;
    paused = false;
    stopped = false;
    externalBest = null;
    externalCheckpoint = null;
    sourceEncodingHash = message.sourceEncodingHash;
    ncm4Session?.free();
    ncm4Mode = profile === "building"
      && (message.format === "ncm3-v1" || message.format === "ncm4-pouw-v1");
    ncm4NeedsTargetValidation = ncm4Mode && Boolean(message.checkpointBase64);
    ncm4Session = ncm4Mode
      ? message.checkpointBase64
        ? BrowserNcm4Session.fromCheckpoint(base64Bytes(message.checkpointBase64))
        : new BrowserNcm4Session(
          profile,
          input,
          BigInt(seed),
          population,
          Number(message.workerId),
          Math.max(1, Number(message.workerCount || 1)),
        )
      : null;
    scheduleSlice();
  } else if (message.type === "pause") {
    paused = true;
  } else if (message.type === "resume") {
    paused = false;
    scheduleSlice();
  } else if (message.type === "stop") {
    stopped = true;
  } else if (message.type === "verifiedBest" && input) {
    const candidate = base64Bytes(message.candidateBase64);
    const report = ncm4Mode
      ? JSON.parse(verify_ncm4_json(profile, input, candidate))
      : JSON.parse(verify_local_json(profile, input, candidate));
    if (report.exact && (!externalBest || better(report, externalBest))) {
      externalBest = report;
      externalCheckpoint = message.checkpointBase64 || null;
      injectExternalCheckpointIfReady();
    }
  }
}

function scheduleSlice() {
  if (running || paused || stopped) return;
  running = true;
  setTimeout(runSlice, 0);
}

function runSlice() {
  try {
    if (paused || stopped) return;
    const response = ncm4Mode
      ? JSON.parse(ncm4Session.stepJson(1))
      : checkpoint
        ? JSON.parse(resume_slice_json(checkpoint))
        : JSON.parse(mine_slice_json(profile, input, BigInt(seed), 1, population));
    if (ncm4NeedsTargetValidation) {
      const report = JSON.parse(verify_ncm4_json(
        profile,
        input,
        base64Bytes(response.candidateBase64),
      ));
      if (!report.exact || response.sourceEncodingHash !== sourceEncodingHash) {
        throw new Error("NCM4 checkpoint target or incumbent encoding does not match the current input.");
      }
      ncm4NeedsTargetValidation = false;
    }
    checkpoint = base64Bytes(response.checkpointBase64);
    injectExternalCheckpointIfReady();
    if (externalBest && better(externalBest, response)) {
      response.sharedVerifiedBestBytes = externalBest.candidateBytes;
    }
    self.postMessage({ type: "result", workerId, result: response });
  } catch (error) {
    self.postMessage({ type: "error", workerId, error: String(error?.message || error) });
    stopped = true;
  } finally {
    running = false;
    if (!paused && !stopped) scheduleSlice();
  }
}

function injectExternalCheckpointIfReady() {
  if (!checkpoint || !externalCheckpoint) return;
  if (ncm4Mode) {
    ncm4Session.injectCheckpoint(base64Bytes(externalCheckpoint));
    checkpoint = ncm4Session.checkpointBytes();
  } else {
    checkpoint = migrate_checkpoint_elite(checkpoint, base64Bytes(externalCheckpoint));
  }
  externalCheckpoint = null;
}

function better(candidate, current) {
  if (candidate.candidateBytes !== current.candidateBytes) return candidate.candidateBytes < current.candidateBytes;
  return candidate.decodeUnits < current.decodeUnits;
}

function respond(result, requestId) {
  self.postMessage({ type: "response", requestId, result });
}

async function ensureInitialized() {
  initialized ||= init();
  await initialized;
}

function base64Bytes(value) {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
