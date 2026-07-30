import { ChunkManager } from "nicechunk-chunk-runtime/chunk/chunk-manager.js";
import { createBuildingChunkMeshes } from "nicechunk-chunk-runtime/construction/building-mesher.js";
import { createBuildingPlacement, parseNcm3Building } from "nicechunk-chunk-runtime/construction/building-parser.js";
import { loadPeasantGuyAvatarMesh } from "nicechunk-chunk-runtime/renderer/avatar-mesh.js";
import { createCameraState } from "nicechunk-chunk-runtime/renderer/camera.js";
import {
  createEquipmentModelParts,
  EQUIPMENT_MODEL_ID,
  equipmentModelBounds,
} from "nicechunk-chunk-runtime/renderer/equipment-model.js";
import { WebGL2VoxelRenderer } from "nicechunk-chunk-runtime/renderer/webgl2-renderer.js";
import { BLOCK_ID, MATERIAL_ID } from "nicechunk-chunk-runtime/world/block-registry.js";
import {
  createWorldGeneratorConfig,
  DEFAULT_GENERATION_VERSION,
  MAINNET_WORLD_SEED,
  surfaceBlockAt,
  terrainSurfaceHeight,
  waterLevelAt,
} from "nicechunk-chunk-runtime/world/world-generator.js";

const VIEW_IDS = Object.freeze({ overview: 0, terrain: 1, building: 2, forged: 3, console: 4 });
const PROFILE_VIEWS = Object.freeze({ terrain_delta: "terrain", building: "building", forged_item: "forged" });
const WORLD_CENTER = Object.freeze({ x: -1376, y: 104, z: 48 });
const TERRAIN_BOUNDS = Object.freeze({ minX: -1408, maxX: -1329, minZ: 16, maxZ: 95, baseY: 72 });
const BUILDING_SITE = Object.freeze({ minX: -1395, minZ: 65, surfaceY: 106, width: 24, depth: 18 });
const COTTAGE_NCM3 = "NCM3:ARgWEhkBRAMAAg8ADAE3BAEDDQAKAUYEAgMCCAABRgsCAwYIAAFGBwkDAwEAAUYEAg0NCAABRgQCBAAICAFGEQIEAAgCAUYRAgsACAEBRhECBwABAwFGEQgHAAIDATkEAgMACAABORECAwAIAAE5BAINAAgAATkRAg0ACAABOQMKAg8AAQE5AwoNDwABAUQHAAADAAIBRAgBAQIAAQpGBAsDAAoKRhELAwAKCGADCgIPDAk5AwoCDwwBPg0OBwEEAQFEDBMGAwAD";
const FORGED_PICKAXE_DESIGN_HASH = 0x4e434b32;
const WORLD_CONFIG = createWorldGeneratorConfig({
  worldSeed: MAINNET_WORLD_SEED,
  generationVersion: DEFAULT_GENERATION_VERSION,
});
const AVATAR_HEIGHT_BLOCKS = 1.75 / 0.4;
const AVATAR_VISUAL_SCALE = AVATAR_HEIGHT_BLOCKS / 2.52;
const TERRAIN_VIEW_DISTANCE = 2;
const RENDER_VIEW_DISTANCE = 5;
const CAMERA_TRANSITION_MS = 1_150;

const MINING_TARGET = Object.freeze({
  x: -1332,
  y: terrainSurfaceHeight(WORLD_CONFIG, -1332, 60),
  z: 60,
});
const ACTORS = Object.freeze({
  miner: actorAt(-1334, 55, Math.PI + Math.atan2(MINING_TARGET.x + 1334, MINING_TARGET.z - 55)),
  builder: actorAt(-1383, 61, Math.PI),
});
const FORGED_ITEM = Object.freeze({
  x: -1344.5,
  y: terrainSurfaceHeight(WORLD_CONFIG, -1344, 64) + 7.2,
  z: 64.5,
});
const EXTRA_TREE_SITES = Object.freeze([
  [-1402, 47], [-1398, 52], [-1380, 45], [-1368, 43], [-1360, 55],
  [-1338, 80], [-1342, 90], [-1370, 92], [-1404, 91], [-1353, 88],
]);

const CAMERA_PRESETS = Object.freeze({
  overview: Object.freeze({
    eye: [-1337, 146, 2],
    target: [-1371, 101, 51],
    fov: 50,
  }),
  terrain: Object.freeze({
    eye: [-1324, 109, 73],
    target: [MINING_TARGET.x, MINING_TARGET.y + 2, MINING_TARGET.z],
    fov: 43,
  }),
  building: Object.freeze({
    eye: [-1356, 126, 107],
    target: [-1383, 113, 75],
    fov: 47,
  }),
  forged: Object.freeze({
    eye: [-1335, 108, 76],
    target: [FORGED_ITEM.x - 2.2, FORGED_ITEM.y, FORGED_ITEM.z + 1.8],
    fov: 35,
  }),
  console: Object.freeze({
    eye: [-1346, 124, 91],
    target: [-1371, 104, 55],
    fov: 44,
  }),
});

export function createMinerWorldScene(canvas, options = {}) {
  if (!(canvas instanceof HTMLCanvasElement)) return createNoopController();

  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const lowPower = Number(navigator.deviceMemory || 8) <= 4 || navigator.connection?.saveData === true;
  const requestedFps = Number(options.maxFps) || (window.innerWidth < 700 ? 22 : 30);
  const maxFps = Math.max(12, Math.min(lowPower ? 20 : 30, requestedFps));
  const frameInterval = 1_000 / maxFps;
  const cleanups = [];
  let renderer = null;
  let chunks = null;
  let cottageChunks = [];
  let terrainSkirtChunks = [];
  let avatars = [];
  let sectionObserver = null;
  let resizeObserver = null;
  let animationFrame = 0;
  let lastFrameTime = 0;
  let startedAt = performance.now();
  let destroyed = false;
  let initialized = false;
  let ready = false;
  let focusView = "overview";
  let scenePhase = "idle";
  let pointerX = 0;
  let pointerY = 0;
  let recentManualFocus = 0;
  let transitionStart = startedAt;
  let cameraStart = cameraPoseForView("overview", canvasAspect(canvas));
  let cameraTarget = cameraStart;
  let lastMiningBurst = -1;
  let lastStats = Object.freeze({
    backend: "chunk.js-webgl2",
    worldSeed: MAINNET_WORLD_SEED,
    generationVersion: DEFAULT_GENERATION_VERSION,
    terrainChunks: 0,
    buildingChunks: 0,
    avatars: 0,
    drawCalls: 0,
    triangles: 0,
    maxFps,
  });

  const schedule = () => {
    if (animationFrame || destroyed || document.hidden) return;
    animationFrame = requestAnimationFrame(renderFrame);
  };

  const focus = (view, { immediate = false, manual = false } = {}) => {
    if (!(view in VIEW_IDS) || destroyed) return;
    const timestamp = performance.now();
    cameraStart = resolveCameraPose(timestamp, false);
    focusView = view;
    cameraTarget = cameraPoseForView(view, canvasAspect(canvas));
    transitionStart = immediate || reducedMotion.matches ? timestamp - CAMERA_TRANSITION_MS : timestamp;
    if (manual) recentManualFocus = timestamp;
    canvas.dataset.sceneView = view;
    updateSceneControls(view);
    renderFrame(timestamp, true);
    schedule();
  };

  const setRunState = (phase) => {
    scenePhase = String(phase || "idle");
    canvas.dataset.scenePhase = scenePhase;
    renderFrame(performance.now(), true);
    schedule();
  };

  const resolveCameraPose = (timestamp, includeParallax = true) => {
    const elapsed = Math.max(0, timestamp - transitionStart);
    const amount = smoothstep(Math.min(1, elapsed / CAMERA_TRANSITION_MS));
    const pose = {
      eye: mixVector(cameraStart.eye, cameraTarget.eye, amount),
      target: mixVector(cameraStart.target, cameraTarget.target, amount),
      fov: mix(cameraStart.fov, cameraTarget.fov, amount),
    };
    if (!includeParallax || reducedMotion.matches) return pose;
    pose.eye[0] += pointerX * 1.45;
    pose.eye[1] -= pointerY * 0.55;
    pose.eye[2] += pointerX * 0.45;
    return pose;
  };

  function renderFrame(timestamp, force = false) {
    animationFrame = 0;
    if (destroyed || document.hidden || !initialized || !renderer || !chunks) return;
    if (!force && timestamp - lastFrameTime < frameInterval) {
      schedule();
      return;
    }

    const dt = Math.min(0.04, Math.max(0.001, (timestamp - lastFrameTime) / 1_000 || 0.016));
    lastFrameTime = timestamp;
    chunks.rebuildDirtyChunks(lowPower ? 2.4 : 4.5);
    updateActors(timestamp);
    renderer.updateVoxelParticles(dt, (worldX, worldZ) => chunks.getOpaqueColumnTopAtWorld(worldX, worldZ) + 1);

    const terrainChunks = [...chunks.chunks.values()].filter((chunk) => chunk.mesh);
    const visibleChunks = terrainChunks.concat(terrainSkirtChunks, cottageChunks);
    const camera = cameraStateFromPose(resolveCameraPose(timestamp), canvasAspect(canvas));
    const activeActors = actorsForView(focusView);
    renderer.prepareChunksForRender(visibleChunks, {
      maxUploads: lowPower ? 2 : 4,
      cameraState: camera,
    });
    const renderStats = renderer.render(camera, visibleChunks, activeActors, overlaysForView(focusView, timestamp));
    const stats = {
      backend: "chunk.js-webgl2",
      worldSeed: MAINNET_WORLD_SEED,
      generationVersion: DEFAULT_GENERATION_VERSION,
      terrainChunks: terrainChunks.length,
      buildingChunks: cottageChunks.length,
      avatars: activeActors.length,
      drawCalls: renderStats.drawCalls || 0,
      triangles: renderStats.triangles || 0,
      maxFps,
    };
    lastStats = Object.freeze(stats);
    canvas.dataset.sceneTerrainChunks = String(stats.terrainChunks);
    canvas.dataset.sceneDrawCalls = String(stats.drawCalls);
    canvas.dataset.sceneTriangles = String(stats.triangles);
    canvas.dataset.sceneActorCount = String(activeActors.length);
    canvas.dataset.sceneActorRoles = activeActors.map((actor) => actor.role).join(",");

    if (!ready && terrainChunks.length) markReady(stats);
    schedule();
  }

  function updateActors(timestamp) {
    if (!avatars.length) return;
    const elapsed = timestamp - startedAt;
    const miningActive = focusView === "terrain" || (focusView === "overview" && scenePhase === "running");
    const miningDuration = scenePhase === "running" ? 820 : 1_350;
    const miningCycle = Math.floor(elapsed / miningDuration);
    const miningProgress = miningActive ? (elapsed % miningDuration) / miningDuration : 0;
    const miner = avatars.find((avatar) => avatar.role === "miner");
    const builder = avatars.find((avatar) => avatar.role === "builder");
    const forgedItem = avatars.find((avatar) => avatar.role === "forged-item");

    if (miner) {
      miner.animation = {
        moving: false,
        miningProgress,
        miningAimPitch: -0.08,
        timeMs: timestamp,
        equipment: { rightHand: "pickaxe" },
      };
    }
    if (builder) {
      builder.animation = {
        moving: focusView === "building" && scenePhase === "running",
        timeMs: timestamp,
        equipment: { rightHand: "blueprint" },
      };
    }
    if (forgedItem) {
      forgedItem.yaw = reducedMotion.matches
        ? -0.62
        : -0.62 + Math.sin(elapsed * 0.00034) * 0.34;
      forgedItem.localOffsetY = FORGED_ITEM.y - Math.floor(FORGED_ITEM.y)
        + (reducedMotion.matches ? 0 : Math.sin(elapsed * 0.00125) * 0.18);
    }

    if (miningActive && miningProgress > 0.58 && miningCycle !== lastMiningBurst) {
      lastMiningBurst = miningCycle;
      const blockId = chunks.getBlockAtWorld(MINING_TARGET.x, MINING_TARGET.y, MINING_TARGET.z);
      renderer.emitVoxelParticles("fracture", {
        worldX: MINING_TARGET.x,
        worldY: MINING_TARGET.y,
        worldZ: MINING_TARGET.z,
        blockId,
        maxPieces: lowPower ? 10 : 18,
      });
    }
  }

  function actorsForView(view) {
    if (view === "terrain") return avatars.filter((avatar) => avatar.role === "miner");
    if (view === "building") return avatars.filter((avatar) => avatar.role === "builder");
    if (view === "forged") return avatars.filter((avatar) => avatar.role === "forged-item");
    if (view === "console") return avatars.filter((avatar) => avatar.role === "miner");
    return avatars;
  }

  function overlaysForView(view, timestamp) {
    if (view === "terrain") {
      const pulse = reducedMotion.matches ? 0.28 : 0.2 + (Math.sin(timestamp * 0.004) + 1) * 0.08;
      return [{
        worldX: MINING_TARGET.x,
        worldY: MINING_TARGET.y,
        worldZ: MINING_TARGET.z,
        size: 1,
        expand: 0.025,
        fillColor: [0.1, 0.72, 1, pulse * 0.22],
        lineColor: [0.28, 0.9, 1, 0.88],
      }];
    }
    if (view === "building") {
      return [{
        shape: "foundation",
        worldX: BUILDING_SITE.minX,
        worldY: BUILDING_SITE.surfaceY + 0.015,
        worldZ: BUILDING_SITE.minZ,
        width: BUILDING_SITE.width,
        depth: BUILDING_SITE.depth,
        preview: true,
        grid: true,
        fillColor: [0.04, 0.42, 0.72, 0.09],
        gridColor: [0.28, 0.86, 1, 0.28],
        edgeColor: [0.58, 0.96, 1, 0.82],
        glowColor: [0.06, 0.68, 1, 0.22],
      }];
    }
    if (view === "forged") {
      return [{
        worldX: FORGED_ITEM.x - 2,
        worldY: FORGED_ITEM.y - 2.1,
        worldZ: FORGED_ITEM.z - 2,
        sizeX: 4,
        sizeY: 4.2,
        sizeZ: 4,
        fillColor: [0.76, 0.94, 0.16, 0.018],
        lineColor: [0.76, 0.94, 0.16, 0.42],
      }];
    }
    return [];
  }

  function markReady(stats) {
    if (ready || destroyed) return;
    ready = true;
    canvas.dataset.sceneReady = "true";
    canvas.dataset.sceneRenderer = "chunk.js-webgl2";
    canvas.dataset.sceneSeed = MAINNET_WORLD_SEED;
    canvas.dataset.sceneGeneration = String(DEFAULT_GENERATION_VERSION);
    canvas.dataset.sceneAvatar = "NCM:peasant_guy:v1";
    canvas.dataset.sceneCottage = "NCM3:house-blueprint";
    canvas.dataset.sceneForgeItem = `forged-pickaxe:${FORGED_PICKAXE_DESIGN_HASH}`;
    canvas.dataset.sceneTerrainThickness = String(BUILDING_SITE.surfaceY - TERRAIN_BOUNDS.baseY);
    canvas.dataset.sceneDecorations = "trees-grass-flowers";
    document.documentElement.classList.remove("miner-scene-fallback");
    document.documentElement.classList.add("miner-scene-ready");
    window.dispatchEvent(new CustomEvent("nicechunk:minersceneready", { detail: stats }));
  }

  async function initialize() {
    try {
      await nextFrame();
      if (destroyed) return;
      renderer = new WebGL2VoxelRenderer(canvas, {
        viewDistance: RENDER_VIEW_DISTANCE,
        textureTileSize: 32,
        textureSeed: MAINNET_WORLD_SEED,
        useRegionBatching: false,
        maxChunkUploadsPerFrame: lowPower ? 2 : 4,
        maxMobileDpr: 1,
        maxDesktopDpr: lowPower ? 1 : 1.25,
        cloudHeight: 126,
        cloudRadius: 420,
        cloudCellSize: lowPower ? 56 : 42,
        cloudFarPadding: 96,
        maxVoxelParticles: lowPower ? 64 : 128,
      });
      renderer.init();

      chunks = new ChunkManager({
        worldSeed: MAINNET_WORLD_SEED,
        generationVersion: DEFAULT_GENERATION_VERSION,
        viewDistance: TERRAIN_VIEW_DISTANCE,
        preloadMargin: 0,
        useWorkers: false,
        visibilityLingerFrames: 0,
      });
      chunks.updatePlayerPosition(WORLD_CENTER.x, WORLD_CENTER.y, WORLD_CENTER.z, { directionX: 0.2, directionZ: -1 });
      chunks.applyPendingDelta(createSceneDecorationDeltas(), "miner-scene-presentation");

      const building = createMinerCottage();
      const placement = createBuildingPlacement(building, {
        id: "miner-cottage-foundation",
        minX: BUILDING_SITE.minX,
        minZ: BUILDING_SITE.minZ,
        surfaceY: BUILDING_SITE.surfaceY,
        width: BUILDING_SITE.width,
        depth: BUILDING_SITE.depth,
      }, { placementId: "miner-cottage", quarterTurns: 0 });
      cottageChunks = createBuildingChunkMeshes(placement, { chunkSize: 16, revision: 1 });
      terrainSkirtChunks = createBuildingChunkMeshes(createTerrainSkirtPlacement(), { chunkSize: 16, revision: 1 });

      const avatarMesh = await loadPeasantGuyAvatarMesh({
        scale: AVATAR_VISUAL_SCALE,
        attachIronPickaxe: true,
      });
      if (destroyed) return;
      renderer.uploadAvatarMesh("peasant-guy", avatarMesh);
      renderer.uploadAvatarMesh("forged-pickaxe", createStandalonePickaxeMesh());
      avatars = [
        createAvatar("miner", ACTORS.miner),
        createAvatar("builder", ACTORS.builder),
        createForgedItemActor(),
      ];

      chunks.rebuildDirtyChunks(lowPower ? 7 : 12);
      initialized = true;
      startedAt = performance.now();
      lastFrameTime = startedAt;
      renderFrame(startedAt, true);
      schedule();
    } catch (error) {
      renderer?.dispose();
      chunks?.dispose();
      renderer = null;
      chunks = null;
      document.documentElement.classList.remove("miner-scene-ready");
      document.documentElement.classList.add("miner-scene-fallback");
      console.warn("NiceChunk Chunk.js scene initialization failed; using the static fallback.", error);
    }
  }

  const handleVisibility = () => {
    if (document.hidden) {
      if (animationFrame) cancelAnimationFrame(animationFrame);
      animationFrame = 0;
      return;
    }
    lastFrameTime = performance.now();
    schedule();
  };
  const handlePointerMove = (event) => {
    if (event.pointerType && event.pointerType !== "mouse") return;
    pointerX = (event.clientX / Math.max(window.innerWidth, 1) - 0.5) * 2;
    pointerY = (event.clientY / Math.max(window.innerHeight, 1) - 0.5) * 2;
  };
  const handleProfile = (event) => focus(PROFILE_VIEWS[event.detail?.profile] || "overview", { manual: true });
  const handlePhase = (event) => setRunState(event.detail?.phase || "idle");

  resizeObserver = new ResizeObserver(() => {
    if (destroyed) return;
    cameraTarget = cameraPoseForView(focusView, canvasAspect(canvas));
    renderer?.resize();
    renderFrame(performance.now(), true);
  });
  resizeObserver.observe(canvas);
  sectionObserver = createSectionObserver((view) => {
    if (performance.now() - recentManualFocus < 900) return;
    const resolved = view === "profile"
      ? PROFILE_VIEWS[document.querySelector("[data-profile][aria-selected='true']")?.dataset.profile] || "terrain"
      : view;
    if (resolved !== focusView) focus(resolved);
  });
  bindSceneControls((view) => focus(view, { manual: true }), cleanups);
  document.addEventListener("visibilitychange", handleVisibility);
  window.addEventListener("pointermove", handlePointerMove, { passive: true });
  window.addEventListener("nicechunk:minerprofile", handleProfile);
  window.addEventListener("nicechunk:minerphase", handlePhase);
  reducedMotion.addEventListener?.("change", handleVisibility);
  canvas.dataset.sceneView = focusView;
  canvas.dataset.scenePhase = scenePhase;
  updateSceneControls(focusView);
  void initialize();

  return {
    focus,
    setRunState,
    get stats() {
      return lastStats;
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      if (animationFrame) cancelAnimationFrame(animationFrame);
      resizeObserver?.disconnect();
      sectionObserver?.disconnect();
      cleanups.forEach((cleanup) => cleanup());
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("nicechunk:minerprofile", handleProfile);
      window.removeEventListener("nicechunk:minerphase", handlePhase);
      reducedMotion.removeEventListener?.("change", handleVisibility);
      renderer?.dispose();
      chunks?.dispose();
      renderer = null;
      chunks = null;
      document.documentElement.classList.remove("miner-scene-ready");
    },
  };
}

/**
 * Render the canonical building semantics returned by pouw-core/WASM.
 * This deliberately does not decode NCM in JavaScript: the browser only turns
 * the already verified coordinate/material map into Chunk.js mesh input.
 */
export function createNcmPreviewScene(canvas, options = {}) {
  if (!(canvas instanceof HTMLCanvasElement)) return createNoopPreviewController();

  const frame = canvas.closest(".ncm-preview-frame");
  const lowPower = Number(navigator.deviceMemory || 8) <= 4 || navigator.connection?.saveData === true;
  const maxFps = lowPower ? 12 : 18;
  const frameInterval = 1_000 / maxFps;
  let renderer = null;
  let chunks = [];
  let bounds = null;
  let inspection = null;
  let initialization = null;
  let resizeObserver = null;
  let visibilityObserver = null;
  let animationFrame = 0;
  let lastFrameTime = 0;
  let visible = true;
  let destroyed = false;
  let revision = 0;

  canvas.dataset.previewReady = "false";
  canvas.dataset.previewRenderer = "chunk.js-webgl2";

  const setFrameState = (value) => {
    if (frame) frame.dataset.previewState = value;
  };

  const schedule = () => {
    if (animationFrame || destroyed || document.hidden || !visible || !renderer || !bounds || !chunks.length) return;
    animationFrame = requestAnimationFrame(renderFrame);
  };

  const markUnavailable = (error) => {
    if (destroyed) return;
    if (animationFrame) cancelAnimationFrame(animationFrame);
    animationFrame = 0;
    canvas.dataset.previewReady = "false";
    canvas.dataset.previewError = String(error?.message || error || "WebGL2 unavailable").slice(0, 180);
    setFrameState("error");
    renderer?.dispose();
    renderer = null;
    options.onUnavailable?.(error);
    console.warn("NiceChunk NCM model preview is unavailable; showing canonical model data instead.", error);
  };

  async function initialize() {
    if (renderer || destroyed) return;
    try {
      await nextFrame();
      if (destroyed) return;
      renderer = new WebGL2VoxelRenderer(canvas, {
        viewDistance: 128,
        textureTileSize: 32,
        textureSeed: "nicechunk-ncm-preview-v1",
        useRegionBatching: false,
        maxChunkUploadsPerFrame: lowPower ? 8 : 20,
        maxMobileDpr: 1,
        maxDesktopDpr: lowPower ? 1 : 1.25,
        cloudHeight: 384,
        cloudRadius: 320,
        cloudCellSize: 64,
        cloudFarPadding: 64,
        maxVoxelParticles: 0,
        clearColor: [0.035, 0.047, 0.055, 1],
      });
      renderer.init();
      delete canvas.dataset.previewError;
      schedule();
    } catch (error) {
      markUnavailable(error);
    }
  }

  function renderFrame(timestamp, force = false) {
    animationFrame = 0;
    if (destroyed || document.hidden || !visible || !renderer || !bounds || !chunks.length) return;
    if (!force && timestamp - lastFrameTime < frameInterval) {
      schedule();
      return;
    }
    lastFrameTime = timestamp;
    try {
      const camera = previewCameraState(bounds, canvasAspect(canvas));
      renderer.prepareChunksForRender(chunks, {
        maxUploads: Math.max(8, chunks.length * 2),
        cameraState: camera,
      });
      const stats = renderer.render(camera, chunks, [], [previewFoundationOverlay(bounds)]);
      canvas.dataset.previewReady = "true";
      canvas.dataset.previewChunks = String(chunks.length);
      canvas.dataset.previewTriangles = String(stats.triangles || 0);
      setFrameState("ready");
    } catch (error) {
      markUnavailable(error);
    }
  }

  function setInspection(nextInspection) {
    revision += 1;
    inspection = nextInspection || null;
    chunks = [];
    bounds = null;
    canvas.dataset.previewReady = "false";
    canvas.dataset.previewProfile = String(inspection?.semantics?.profile || "");
    canvas.dataset.previewFormat = String(inspection?.format || "");
    canvas.dataset.previewSemanticRoot = String(inspection?.semanticRoot || "");
    canvas.dataset.previewVoxelCount = String(inspection?.voxelCount ?? "");
    delete canvas.dataset.previewDimensions;
    renderer?.pruneChunks(new Set());

    if (inspection?.semantics?.profile !== "building") {
      setFrameState(inspection ? "unsupported" : "waiting");
      return;
    }

    try {
      const currentRevision = revision;
      const placement = createCanonicalBuildingPlacement(inspection);
      chunks = createBuildingChunkMeshes(placement, { chunkSize: 16, revision: currentRevision });
      bounds = placement.bounds;
      const size = inspection.semantics.semantics.size;
      canvas.dataset.previewDimensions = size.join("x");
      canvas.dataset.previewChunks = String(chunks.length);
      setFrameState(chunks.length ? "loading" : "empty");
      if (!chunks.length) return;
      if (renderer) {
        renderer.pruneChunks(new Set(chunks.map((chunk) => chunk.id)));
        renderFrame(performance.now(), true);
      } else if (!initialization) {
        initialization = initialize().finally(() => {
          initialization = null;
        });
      }
    } catch (error) {
      markUnavailable(error);
    }
  }

  const handleVisibility = () => {
    if (document.hidden) {
      if (animationFrame) cancelAnimationFrame(animationFrame);
      animationFrame = 0;
      return;
    }
    lastFrameTime = 0;
    schedule();
  };

  resizeObserver = new ResizeObserver(() => {
    renderer?.resize();
    renderFrame(performance.now(), true);
  });
  resizeObserver.observe(canvas);
  if ("IntersectionObserver" in window) {
    visibilityObserver = new IntersectionObserver((entries) => {
      visible = entries.some((entry) => entry.isIntersecting && entry.intersectionRatio > 0);
      if (!visible && animationFrame) cancelAnimationFrame(animationFrame);
      if (!visible) animationFrame = 0;
      else schedule();
    }, { rootMargin: "160px 0px", threshold: [0, 0.01] });
    visibilityObserver.observe(canvas);
  }
  document.addEventListener("visibilitychange", handleVisibility);

  return {
    setInspection,
    destroy() {
      if (destroyed) return;
      destroyed = true;
      if (animationFrame) cancelAnimationFrame(animationFrame);
      resizeObserver?.disconnect();
      visibilityObserver?.disconnect();
      document.removeEventListener("visibilitychange", handleVisibility);
      renderer?.dispose();
      renderer = null;
      chunks = [];
      bounds = null;
    },
  };
}

function createCanonicalBuildingPlacement(inspection) {
  const semantics = inspection?.semantics?.semantics;
  const size = semantics?.size;
  if (!Array.isArray(size) || size.length !== 3 || size.some((value) => !Number.isSafeInteger(value) || value <= 0)) {
    throw new Error("Canonical building dimensions are invalid.");
  }
  if (!Array.isArray(semantics.voxels)) throw new Error("Canonical building voxels are unavailable.");

  const [sizeX, sizeY, sizeZ] = size;
  const voxels = new Map();
  for (const voxel of semantics.voxels) {
    const x = Number(voxel?.x);
    const y = Number(voxel?.y);
    const z = Number(voxel?.z);
    const material = Number(voxel?.material);
    if (![x, y, z, material].every(Number.isSafeInteger)
      || x < 0 || x >= sizeX || y < 0 || y >= sizeY || z < 0 || z >= sizeZ || material <= 0) {
      throw new Error("Canonical building contains an invalid voxel.");
    }
    voxels.set(`${x},${y},${z}`, { x, y, z, material });
  }
  if (voxels.size !== semantics.voxels.length) throw new Error("Canonical building contains duplicate voxels.");

  const rootId = String(inspection.semanticRoot || "unrooted").slice(0, 16);
  const building = {
    id: `canonical-${rootId}`,
    format: "NCM3",
    codeId: rootId,
    size: { x: sizeX, y: sizeY, z: sizeZ },
    voxels,
    voxelCount: voxels.size,
    scale: 1,
  };
  return createBuildingPlacement(building, {
    id: `preview-foundation-${rootId}`,
    minX: 0,
    minZ: 0,
    surfaceY: 0,
    width: sizeX,
    depth: sizeZ,
  }, { placementId: `ncm-preview-${rootId}` });
}

function previewCameraState(bounds, aspect) {
  const target = [
    (bounds.minX + bounds.maxX + 1) * 0.5,
    (bounds.minY + bounds.maxY + 1) * 0.5,
    (bounds.minZ + bounds.maxZ + 1) * 0.5,
  ];
  const radius = Math.max(2.4, Math.hypot(bounds.width, bounds.height, bounds.depth) * 0.52);
  const narrowScale = Math.max(1, 0.9 / Math.max(0.32, aspect));
  const distance = radius * 3.05 * narrowScale;
  const angle = 0.73;
  const horizontal = distance * 0.82;
  const eye = [
    target[0] + Math.cos(angle) * horizontal,
    target[1] + distance * 0.48,
    target[2] + Math.sin(angle) * horizontal,
  ];
  return cameraStateFromPose({ eye, target, fov: 39 }, aspect, Math.max(128, distance * 2.4));
}

function previewFoundationOverlay(bounds) {
  return {
    shape: "foundation",
    worldX: bounds.minX,
    worldY: bounds.minY - 0.02,
    worldZ: bounds.minZ,
    width: bounds.width,
    depth: bounds.depth,
    preview: true,
    grid: true,
    fillColor: [0.04, 0.42, 0.72, 0.07],
    gridColor: [0.28, 0.86, 1, 0.2],
    edgeColor: [0.58, 0.96, 1, 0.68],
    glowColor: [0.06, 0.68, 1, 0.14],
  };
}

function createMinerCottage() {
  return parseNcm3Building(COTTAGE_NCM3, {
    id: "miner-cottage",
    name: "Miner Cottage",
  });
}

function createTerrainSkirtPlacement() {
  const voxels = new Map();
  const outer = {
    minX: TERRAIN_BOUNDS.minX - 1,
    maxX: TERRAIN_BOUNDS.maxX + 1,
    minZ: TERRAIN_BOUNDS.minZ - 1,
    maxZ: TERRAIN_BOUNDS.maxZ + 1,
  };
  const put = (x, y, z, material) => voxels.set(`${x},${y},${z}`, { x, y, z, material });
  const edgeHeight = (x, z) => terrainSurfaceHeight(
    WORLD_CONFIG,
    Math.max(TERRAIN_BOUNDS.minX, Math.min(TERRAIN_BOUNDS.maxX, x)),
    Math.max(TERRAIN_BOUNDS.minZ, Math.min(TERRAIN_BOUNDS.maxZ, z)),
  );

  for (let x = outer.minX; x <= outer.maxX; x += 1) {
    appendRockWall(put, x, outer.minZ, edgeHeight(x, outer.minZ));
    appendRockWall(put, x, outer.maxZ, edgeHeight(x, outer.maxZ));
  }
  for (let z = outer.minZ + 1; z < outer.maxZ; z += 1) {
    appendRockWall(put, outer.minX, z, edgeHeight(outer.minX, z));
    appendRockWall(put, outer.maxX, z, edgeHeight(outer.maxX, z));
  }
  for (let z = outer.minZ; z <= outer.maxZ; z += 1) {
    for (let x = outer.minX; x <= outer.maxX; x += 1) put(x, TERRAIN_BOUNDS.baseY, z, MATERIAL_ID.basalt);
  }

  // The real cottage keeps its one-voxel scale while a stone plinth follows the uneven terrain below it.
  for (let z = BUILDING_SITE.minZ; z < BUILDING_SITE.minZ + BUILDING_SITE.depth; z += 1) {
    for (let x = BUILDING_SITE.minX; x < BUILDING_SITE.minX + BUILDING_SITE.width; x += 1) {
      const surface = terrainSurfaceHeight(WORLD_CONFIG, x, z);
      for (let y = surface + 1; y < BUILDING_SITE.surfaceY; y += 1) put(x, y, z, MATERIAL_ID.cobblestone);
    }
  }

  let maxY = TERRAIN_BOUNDS.baseY;
  for (const voxel of voxels.values()) maxY = Math.max(maxY, voxel.y);
  return {
    id: "miner-terrain-pedestal",
    worldVoxels: voxels,
    voxelCount: voxels.size,
    bounds: {
      minX: outer.minX,
      minY: TERRAIN_BOUNDS.baseY,
      minZ: outer.minZ,
      maxX: outer.maxX,
      maxY,
      maxZ: outer.maxZ,
      width: outer.maxX - outer.minX + 1,
      height: maxY - TERRAIN_BOUNDS.baseY + 1,
      depth: outer.maxZ - outer.minZ + 1,
    },
  };
}

function appendRockWall(put, x, z, topY) {
  for (let y = TERRAIN_BOUNDS.baseY; y <= topY; y += 1) {
    const depth = y - TERRAIN_BOUNDS.baseY;
    const material = depth < 6 ? MATERIAL_ID.basalt : (depth < 16 ? MATERIAL_ID.deepStone : MATERIAL_ID.stone);
    put(x, y, z, material);
  }
}

function createSceneDecorationDeltas() {
  const blocks = new Map();
  const put = (worldX, worldY, worldZ, blockId) => {
    blocks.set(`${worldX},${worldY},${worldZ}`, { worldX, worldY, worldZ, blockId });
  };

  put(MINING_TARGET.x, MINING_TARGET.y, MINING_TARGET.z, BLOCK_ID.coal);

  for (let index = 0; index < EXTRA_TREE_SITES.length; index += 1) {
    const [x, z] = EXTRA_TREE_SITES[index];
    const surface = terrainSurfaceHeight(WORLD_CONFIG, x, z);
    const water = waterLevelAt(WORLD_CONFIG, x, z, surface);
    if (water !== null && water > surface) continue;
    appendPresentationTree(put, x, surface + 1, z, 4 + (index % 3));
  }

  for (let z = TERRAIN_BOUNDS.minZ + 4; z <= TERRAIN_BOUNDS.maxZ - 4; z += 3) {
    for (let x = TERRAIN_BOUNDS.minX + 4; x <= TERRAIN_BOUNDS.maxX - 4; x += 3) {
      const hash = sceneHash(x, z);
      if (hash % 100 >= 28 || presentationExclusion(x, z)) continue;
      const surface = terrainSurfaceHeight(WORLD_CONFIG, x, z);
      const water = waterLevelAt(WORLD_CONFIG, x, z, surface);
      if ((water !== null && water > surface) || surfaceBlockAt(WORLD_CONFIG, x, z, surface) !== BLOCK_ID.grass) continue;
      const variants = [BLOCK_ID.grassPlant, BLOCK_ID.grassPlant, BLOCK_ID.flowerWhite, BLOCK_ID.flowerYellow, BLOCK_ID.flowerRed, BLOCK_ID.flowerBlue, BLOCK_ID.flowerPink];
      put(x, surface + 1, z, variants[(hash >>> 8) % variants.length]);
    }
  }
  return [...blocks.values()];
}

function appendPresentationTree(put, x, baseY, z, height) {
  for (let y = 0; y < height; y += 1) put(x, baseY + y, z, BLOCK_ID.trunk);
  const crownY = baseY + height - 1;
  for (let dy = -1; dy <= 2; dy += 1) {
    const radius = dy === 2 ? 1 : 2;
    for (let dz = -radius; dz <= radius; dz += 1) {
      for (let dx = -radius; dx <= radius; dx += 1) {
        if (Math.abs(dx) === radius && Math.abs(dz) === radius && radius > 1) continue;
        if (dx === 0 && dz === 0 && dy <= 0) continue;
        put(x + dx, crownY + dy, z + dz, BLOCK_ID.leaves);
      }
    }
  }
}

function presentationExclusion(x, z) {
  if (x >= BUILDING_SITE.minX - 3 && x < BUILDING_SITE.minX + BUILDING_SITE.width + 3
    && z >= BUILDING_SITE.minZ - 3 && z < BUILDING_SITE.minZ + BUILDING_SITE.depth + 3) return true;
  return EXTRA_TREE_SITES.some(([treeX, treeZ]) => Math.abs(treeX - x) <= 3 && Math.abs(treeZ - z) <= 3)
    || Math.hypot(x - MINING_TARGET.x, z - MINING_TARGET.z) < 5
    || Math.hypot(x - FORGED_ITEM.x, z - FORGED_ITEM.z) < 5;
}

function sceneHash(x, z) {
  let value = Math.imul(Math.trunc(x), 0x45d9f3b) ^ Math.imul(Math.trunc(z), 0x119de1f3) ^ 0x4e434b;
  value = Math.imul(value ^ (value >>> 16), 0x45d9f3b);
  return (value ^ (value >>> 16)) >>> 0;
}

function createStandalonePickaxeMesh() {
  const gameParts = createEquipmentModelParts(EQUIPMENT_MODEL_ID.forgedPickaxe, {
    designHash: FORGED_PICKAXE_DESIGN_HASH,
  });
  const parts = createDisplayPickaxeParts(gameParts);
  const sourceBounds = equipmentModelBounds(parts);
  const sourceCenterZ = (sourceBounds.minZ + sourceBounds.maxZ) * 0.5;
  const vertices = [];
  const indices = [];
  const scale = 1.9;
  for (const part of parts) {
    appendPickaxeCuboid(vertices, indices, {
      center: [
        part.center[1] * scale,
        (-part.center[2] + sourceCenterZ) * scale,
        part.center[0] * scale,
      ],
      size: [part.size[1] * scale, part.size[2] * scale, part.size[0] * scale],
      color: part.color,
    });
  }
  return {
    name: "forged-pickaxe",
    vertices: new Float32Array(vertices),
    indices: new Uint16Array(indices),
    vertexCount: vertices.length / 10,
    indexCount: indices.length,
    triangleCount: indices.length / 3,
    vertexStrideBytes: 40,
  };
}

function createDisplayPickaxeParts(gameParts) {
  const metal = gameParts.find((part) => part.name === "toolHead")?.color ?? [0.58, 0.64, 0.68, 1];
  const highlight = gameParts.find((part) => part.name === "toolRune")?.color ?? [0.52, 0.9, 1, 1];
  const core = gameParts.filter((part) => part.name !== "toolTipTop" && part.name !== "toolTipBottom");
  return core.concat([
    displayPickaxePart("leftShoulder", [0, -0.66, -1.06], [0.26, 0.38, 0.22], metal),
    displayPickaxePart("leftBlade", [0, -0.98, -1.01], [0.21, 0.30, 0.18], metal),
    displayPickaxePart("leftTip", [0, -1.21, -0.93], [0.14, 0.20, 0.14], highlight),
    displayPickaxePart("rightShoulder", [0, 0.66, -1.06], [0.26, 0.38, 0.22], metal),
    displayPickaxePart("rightBlade", [0, 0.98, -1.01], [0.21, 0.30, 0.18], metal),
    displayPickaxePart("rightTip", [0, 1.21, -0.93], [0.14, 0.20, 0.14], highlight),
    displayPickaxePart("forgeInlayLeft", [-0.15, -0.38, -1.22], [0.06, 0.30, 0.06], highlight),
    displayPickaxePart("forgeInlayRight", [-0.15, 0.38, -1.22], [0.06, 0.30, 0.06], highlight),
  ]);
}

function displayPickaxePart(name, center, size, color) {
  return { name, center, size, color };
}

function appendPickaxeCuboid(vertices, indices, part) {
  const [cx, cy, cz] = part.center;
  const [sx, sy, sz] = part.size.map((value) => value * 0.5);
  const x0 = cx - sx;
  const x1 = cx + sx;
  const y0 = cy - sy;
  const y1 = cy + sy;
  const z0 = cz - sz;
  const z1 = cz + sz;
  const faces = [
    [[1, 0, 0], [[x1, y0, z1], [x1, y1, z1], [x1, y1, z0], [x1, y0, z0]]],
    [[-1, 0, 0], [[x0, y0, z0], [x0, y1, z0], [x0, y1, z1], [x0, y0, z1]]],
    [[0, 1, 0], [[x0, y1, z1], [x0, y1, z0], [x1, y1, z0], [x1, y1, z1]]],
    [[0, -1, 0], [[x0, y0, z0], [x0, y0, z1], [x1, y0, z1], [x1, y0, z0]]],
    [[0, 0, 1], [[x0, y0, z1], [x0, y1, z1], [x1, y1, z1], [x1, y0, z1]]],
    [[0, 0, -1], [[x1, y0, z0], [x1, y1, z0], [x0, y1, z0], [x0, y0, z0]]],
  ];
  for (const [normal, points] of faces) {
    const offset = vertices.length / 10;
    const tiltedNormal = tiltPickaxeVector(normal);
    for (const point of points) vertices.push(...tiltPickaxeVector(point), ...tiltedNormal, ...part.color);
    indices.push(offset, offset + 1, offset + 2, offset, offset + 2, offset + 3);
  }
}

function tiltPickaxeVector([x, y, z]) {
  const tilt = -0.12;
  const cosine = Math.cos(tilt);
  const sine = Math.sin(tilt);
  return [x * cosine - y * sine, x * sine + y * cosine, z];
}

function createForgedItemActor() {
  const worldX = Math.floor(FORGED_ITEM.x);
  const worldY = Math.floor(FORGED_ITEM.y);
  const worldZ = Math.floor(FORGED_ITEM.z);
  return {
    id: "forged-item",
    meshId: "forged-pickaxe",
    role: "forged-item",
    worldX,
    worldY,
    worldZ,
    localOffsetX: FORGED_ITEM.x - worldX,
    localOffsetY: FORGED_ITEM.y - worldY,
    localOffsetZ: FORGED_ITEM.z - worldZ,
    yaw: 0,
    alwaysVisible: true,
    cullRadius: 7,
    shadowWorldY: terrainSurfaceHeight(WORLD_CONFIG, worldX, worldZ) + 1,
    shadowCasterHeight: 6,
    shadowRadiusX: 1.2,
    shadowRadiusZ: 1.2,
    shadowAlpha: 0.34,
  };
}

function createAvatar(role, actor) {
  return {
    id: "peasant-guy",
    meshId: "peasant-guy",
    role,
    worldX: actor.x,
    worldY: actor.y,
    worldZ: actor.z,
    localOffsetX: 0.5,
    localOffsetY: 0,
    localOffsetZ: 0.5,
    yaw: actor.yaw,
    animation: { moving: false, timeMs: performance.now() },
    shadowWorldY: actor.y,
    shadowCasterHeight: AVATAR_HEIGHT_BLOCKS,
    shadowRadiusX: 0.55,
    shadowRadiusZ: 0.45,
    shadowAlpha: 0.42,
  };
}

function actorAt(x, z, yaw) {
  return Object.freeze({
    x,
    y: terrainSurfaceHeight(WORLD_CONFIG, x, z) + 1,
    z,
    yaw,
  });
}

function cameraPoseForView(view, aspect) {
  const source = CAMERA_PRESETS[view] || CAMERA_PRESETS.overview;
  const mobile = aspect < 0.78;
  const distanceScale = mobile ? 1.28 : 1;
  const target = [...source.target];
  if (mobile && view === "terrain") target[1] += 1.4;
  const eye = target.map((value, index) => value + (source.eye[index] - source.target[index]) * distanceScale);
  return {
    eye,
    target,
    fov: source.fov + (mobile ? 4 : 0),
  };
}

function cameraStateFromPose(pose, aspect, far = 520) {
  const eyeX = Math.floor(pose.eye[0]);
  const eyeY = Math.floor(pose.eye[1]);
  const eyeZ = Math.floor(pose.eye[2]);
  const targetX = Math.floor(pose.target[0]);
  const targetY = Math.floor(pose.target[1]);
  const targetZ = Math.floor(pose.target[2]);
  return createCameraState({
    worldX: eyeX,
    worldY: eyeY,
    worldZ: eyeZ,
    localOffsetX: pose.eye[0] - eyeX,
    localOffsetY: pose.eye[1] - eyeY,
    localOffsetZ: pose.eye[2] - eyeZ,
    targetWorldX: targetX,
    targetWorldY: targetY,
    targetWorldZ: targetZ,
    targetLocalOffsetX: pose.target[0] - targetX,
    targetLocalOffsetY: pose.target[1] - targetY,
    targetLocalOffsetZ: pose.target[2] - targetZ,
    fov: pose.fov,
    aspect,
    near: 0.1,
    far,
  });
}

function bindSceneControls(onFocus, cleanups) {
  document.querySelectorAll("[data-scene-view]").forEach((element) => {
    const handler = () => onFocus(element.dataset.sceneView || "overview");
    element.addEventListener("click", handler);
    cleanups.push(() => element.removeEventListener("click", handler));
  });
}

function updateSceneControls(view) {
  document.querySelectorAll("[data-scene-view]").forEach((element) => {
    const active = element.dataset.sceneView === view;
    element.classList.toggle("scene-active", active);
    if (element.matches("button")) element.setAttribute("aria-pressed", String(active));
  });
}

function createSectionObserver(onView) {
  if (!("IntersectionObserver" in window)) return { disconnect() {} };
  const mappings = [
    [document.querySelector(".hero"), "overview"],
    [document.querySelector("#demo"), "profile"],
    [document.querySelector("#how"), "building"],
    [document.querySelector("#downloads"), "forged"],
    [document.querySelector("#spec"), "overview"],
  ].filter(([element]) => element);
  const views = new Map(mappings);
  const ratios = new Map();
  const observer = new IntersectionObserver((entries) => {
    entries.forEach((entry) => ratios.set(entry.target, entry.isIntersecting ? entry.intersectionRatio : 0));
    const active = [...ratios.entries()].sort((left, right) => right[1] - left[1])[0];
    if (active?.[1] > 0.12) onView(views.get(active[0]));
  }, { rootMargin: "-18% 0px -38%", threshold: [0, 0.12, 0.28, 0.5, 0.72] });
  mappings.forEach(([element]) => {
    ratios.set(element, 0);
    observer.observe(element);
  });
  return observer;
}

function canvasAspect(canvas) {
  const rect = canvas.getBoundingClientRect();
  return Math.max(0.25, (rect.width || window.innerWidth || 1) / Math.max(1, rect.height || window.innerHeight || 1));
}

function nextFrame() {
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

function mix(left, right, amount) {
  return left + (right - left) * amount;
}

function mixVector(left, right, amount) {
  return left.map((value, index) => mix(value, right[index], amount));
}

function smoothstep(value) {
  return value * value * (3 - 2 * value);
}

function createNoopController() {
  return {
    focus() {},
    setRunState() {},
    stats: Object.freeze({ backend: "unavailable", terrainChunks: 0, buildingChunks: 0, avatars: 0, drawCalls: 0, triangles: 0, maxFps: 0 }),
    destroy() {},
  };
}

function createNoopPreviewController() {
  return {
    setInspection() {},
    destroy() {},
  };
}
