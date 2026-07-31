# Changelog

All notable Miner changes are documented here. The project uses Semantic
Versioning while protocol, VM, cost-model, and codec versions are tracked
independently.

## Unreleased

### Added

- Detect pasted NCM3, NCM4P, and NCF1 inputs in the shared Rust/WASM core and
  automatically select the matching Building or Forged Item mining profile.
- Render canonical building voxels and complete NCF1 component geometry in an
  interactive Chunk.js model with rotate, pan, zoom, keyboard, and reset
  controls.

### Changed

- Present the browser Miner, CLI, README, release notes, and protocol
  documentation as the released deterministic compression product rather than
  a research laboratory or preview.

### Fixed

- Keep parsing, mining, and verification operational when WebGL2 is unavailable
  by degrading only the interactive 3D view to its canonical data summary.

## 0.2.0-alpha.7 - 2026-07-30

### Fixed

- Use the platform-native CUDA `c_char` device-name buffer so the optional CUDA
  crate compiles on Linux ARM64 as well as x86_64.

## 0.2.0-alpha.6 - 2026-07-30

### Added

- Add a native CUDA batch evaluator for all 13 NCM4 Building opcodes while
  retaining independent CPU serialization, decode, semantic-root, and exact
  verification for every promoted survivor.
- Add `gpu-info`, `--accelerator`, CUDA device/batch/survivor controls, and a
  separately identified Linux x86_64 CUDA release archive.
- Persist the resolved evaluator in checkpoint version 2 and migrate version 1
  checkpoints explicitly to CPU.

### Changed

- Let `mine` continue until Ctrl-C by default; generation, time, and attempt
  limits now apply only when supplied explicitly.
- Report the active evaluator, device or fallback reason, and exact stop reason
  in human-readable and JSON mining status.

### Measured

- On the tested RTX 4090, CUDA increased NCM4 attempts/s by 4.15x to 8.39x on
  three Building fixtures while preserving candidate bytes and semantic roots.

## 0.2.0-alpha.5 - 2026-07-30

### Fixed

- Evaluate every island's offspring in one ordered Rayon batch so native
  `--threads` can use cores beyond the configured island count while preserving
  fixed-seed and checkpoint behavior.

## 0.2.0-alpha.4 - 2026-07-30

### Added

- Accept NCM3 `.ncm`/`.ncm3` assets directly in `mine`, honor an explicit
  native `--threads` count, and emit detailed start/search/improvement/complete
  status records on stderr.
- Report source/candidate bytes, savings, byte layout, decode units, semantic
  root, exactness, worker configuration, and throughput whenever a shorter
  NCM4 witness is found.

### Fixed

- Treat missing WebGL2 as an expected graphics capability fallback so the
  static scene, canonical WASM model summary, and CPU mining Workers continue
  without an exception stack or the 3D bundle.

### Changed

- Package cross-platform CLI and Web/WASM release archives only for version
  tags or an explicit manual workflow run; ordinary Git pushes remain
  validation-only.

## 0.2.0-alpha.3 - 2026-07-30

### Fixed

- Make the imported-NCM4 browser stop assertion idempotent when a constrained
  search finishes naturally before Playwright requests Stop.

## 0.2.0-alpha.2 - 2026-07-30

### Fixed

- Give the browser pause/resume smoke test enough search budget on slower CI
  runners so it cannot finish naturally before Playwright clicks Pause.

## 0.2.0-alpha.1 - 2026-07-30

### Added

- Initial NCM4 PoUW format with collision-safe `NC4P` binary magic and
  `NCM4P:` text transport.
- Format dispatcher that maps NCM3, NCM4, ChunkBroken, and NCF1 into shared
  canonical semantics and domain-separated SHA-256 roots.
- Compact Building palette/bitstream with BOX, REPEAT_BOX, GABLE, TREE, FENCE,
  RUN, WALL, EXTRUDE, TRANSLATE, ROTATE_Y, MIRROR, REPEAT_REGION, and CLEAR_BOX.
- Exact residual competition across sparse, runs, boxes, layers, XOR bitmap,
  and material-group encodings.
- Language audit/preflight with real stored-byte breakdown, fixed lower bound,
  deterministic witness, deep-search recommendation, and NCM3 fallback.
- Persistent typed genetic and large-neighborhood search islands, verified
  elite migration, deterministic sharding, full checkpoints, and support for
  more than eight threads/islands.
- NCM4 CLI analyze/encode/decode/verify/mine/resume commands and combined
  PoUW v1/NCM4 benchmark output.
- Long-lived browser WASM sessions, NCM4 input/export, IndexedDB checkpoints,
  verified Worker migration, and NCM4 metrics/history UI.
- Real variant and held-out Building fixtures with strictly shorter NCM4
  witnesses.
- Cross-platform release workflow including Web/WASM, aggregate
  `SHA256SUMS`, pre-release metadata, and an independent property-test job.

### Fixed

- Removed the native eight-island ceiling and repeated Rayon pool creation.
- Preserved browser population and RNG state across generations and resume.
- Made external Worker elites enter populations instead of only changing UI.
- Added type-safe search reachability for every implemented NCM4 opcode.
- Corrected current NCF1 text auto-detection from `NCF1:` to `NCF1.`.
- Replaced optional/stale JavaScript compatibility checks with pinned current
  Game and Chunk.js differential tests.

### Compatibility

- NCM3 bytes, command semantics, decoder behavior, fixtures, and canonical
  semantic roots are unchanged.
- NCM4 never wins unless independent decode is exact and normalized binary
  bytes are strictly fewer; otherwise the source format remains selected.
- Terrain and forged-item NCM4 use exact wrappers in this release. Compact search
  for those profiles, GPU evaluators, pools, wallets, rewards, and an on-chain
  verifier are not included.

## 0.1.3

- Published the initial deterministic PoUW v1 core, CLI, browser miner, release
  packaging, and native/WASM verification workflow.
