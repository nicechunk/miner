import init, {
  baseline_json,
  inspect_json,
  mine_slice_json,
  resume_slice_json,
  verify_local_json,
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

self.addEventListener("message", (event) => {
  handle(event.data).catch((error) => {
    self.postMessage({ type: "error", workerId, error: String(error?.message || error) });
  });
});

async function handle(message) {
  await ensureInitialized();
  if (message.type === "version") {
    respond(JSON.parse(version_json()));
  } else if (message.type === "inspect") {
    respond(JSON.parse(inspect_json(message.profile, message.input)));
  } else if (message.type === "baseline") {
    respond(JSON.parse(baseline_json(message.profile, message.input)));
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
    const report = JSON.parse(verify_local_json(profile, input, candidate));
    if (report.exact && (!externalBest || better(report, externalBest))) externalBest = report;
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
    const response = checkpoint
      ? JSON.parse(resume_slice_json(checkpoint))
      : JSON.parse(mine_slice_json(profile, input, BigInt(seed), 1, population));
    checkpoint = base64Bytes(response.checkpointBase64);
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

function better(candidate, current) {
  if (candidate.candidateBytes !== current.candidateBytes) return candidate.candidateBytes < current.candidateBytes;
  return candidate.decodeUnits < current.decodeUnits;
}

function respond(result) {
  self.postMessage({ type: "response", result });
}

async function ensureInitialized() {
  initialized ||= init();
  await initialized;
}

function base64Bytes(value) {
  const binary = atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
