# Changelog

All notable Miner changes are documented here. The project uses Semantic
Versioning while protocol, VM, cost-model, and codec versions are tracked
independently.

## 0.2.0-alpha.1 - 2026-07-30

### Added

- Experimental NCM4 PoUW format with collision-safe `NC4P` binary magic and
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
- Cross-platform Alpha release workflow including Web/WASM, aggregate
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
- Terrain and forged-item NCM4 use exact wrappers in this Alpha. Compact search
  for those profiles, GPU evaluators, pools, wallets, rewards, and an on-chain
  verifier are not included.

## 0.1.3

- Published the initial deterministic PoUW v1 core, CLI, browser miner, release
  packaging, and native/WASM verification workflow.
