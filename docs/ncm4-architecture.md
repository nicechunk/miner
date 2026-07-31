# NCM4 Architecture

## Compatibility boundary

NCM4 is isolated from game runtime code. The format dispatcher selects one
strict decoder from input magic/prefix, then every path converges on the same
canonical semantic model:

```text
                         +----------------+
NCM3 bytes/text -------->| NCM3 v1 parser |---+
                         +----------------+   |
                         +----------------+   v
NC4P / NCM4P: ---------->| NCM4 decoder   |-> Canonical Semantics -> semanticRoot
                         +----------------+   ^
ChunkBroken / NCF1 ----->| legacy parser  |---+
                         +----------------+
```

No NCM3 opcode or overwrite rule was changed. The NCM3 golden roots are pinned
in tests, and the production Chunk.js decoder is exercised in a differential
test. The dispatcher's `NC4P` magic cannot be interpreted as NCM3.

## Workspace ownership

- `pouw-core` owns format detection, NCM4 binary parsing/serialization,
  canonical semantics, language preflight, hashes, limits, and final decode.
- `pouw-search` owns replaceable search policy, populations, checkpoints,
  deterministic shards, Rayon execution, and candidate deduplication.
- `pouw-cli` owns filesystem I/O, Ctrl-C, progress/report formatting, and atomic
  output. It does not implement a second verifier.
- `pouw-wasm` exposes the same Rust core and a long-lived single-island session
  to browser Workers.
- `web/worker.js` orchestrates bounded slices and verified elite migration.
  `web/app.js` owns UI state, IndexedDB checkpoint persistence, and downloads.

The Rust core remains integer-only and does not access files, network, system
time, or JavaScript APIs. The `no_std + alloc` core check remains in CI.

## Candidate lifecycle

```text
source bytes
  -> strict source decoder
  -> canonical target (computed once)
  -> deterministic NCM4 transcode/structure seed
  -> typed mutation or large-neighborhood rewrite
  -> regenerate complete exact residual
  -> serialize canonical NC4P bytes
  -> independent NC4P decode
  -> compare canonical target and semantic root
  -> compare actual stored bytes
```

Only the independently decoded output can become a session best or migrate to
another island. A searcher's internal AST, fitness claim, byte count, hash, and
exact flag are not consensus evidence.

## Language preflight

Preflight runs before deep search and reports source bytes, all NCM4 byte
components, the fixed lower bound, deterministic seed length, exactness,
witness status, and the selected format. Building deep search is recommended
only when the current language is plausibly competitive. Terrain and forged
items use exact wrappers in version 1 and explicitly return no deep-search
recommendation.

The product rule is monotonic:

```text
if exactNcm4Bytes < sourceBytes:
    select NCM4
else:
    retain source format
```

Equal bytes with fewer decode units may be useful diagnostic information, but
it is not reported as a storage improvement.

## Native execution

`Ncm4SearchSession` constructs one bounded Rayon pool and keeps it for the
session lifetime. `threads` and `islands` are independent u16 settings; there
is no eight-island cap. Each generation injects the verified global elite,
evolves islands in parallel, updates counters, and publishes a snapshot.

The deterministic stream identity combines seed, shard index, island index,
and generation. `--shard-index i --shard-count n` lets machines traverse
different streams without a coordinator. Cross-machine result transport is a
file/interface concern; this release does not pretend to implement a pool.

## Browser execution

The browser uses ordinary Workers so the site does not require COOP/COEP or
`SharedArrayBuffer`. Each Worker owns one WASM instance and a persistent Rust
session. The main thread exchanges only serialized checkpoints and candidates.
Receiving Workers independently validate an external elite and inject it into
their population rather than merely updating a displayed number.

Checkpoint records are stored in IndexedDB under `ncm4-checkpoints-v2`, keyed
by semantic root and incumbent encoding hash. Writes are rate-limited. This
prevents equivalent semantics with different incumbent costs from sharing the
wrong comparison state. Pause keeps Workers and sessions alive; Stop terminates
them promptly; page hiding pauses work.

## Verification and rendering

Native and WASM verification are the same Rust implementation. The web page
shows source format, candidate format, semantic roots, exact mismatch state,
byte components, generation, strategy, attempts, and the actual best-history
curve. It never uploads source bytes.

The current Chunk.js scene provides the existing profile views and the UI
exposes verified source/candidate semantic summaries. Version 1 does not yet
construct two independent voxel meshes plus a per-cell colored difference
overlay. That rendering gap does not weaken exact verification, but it remains
a product limitation rather than being described as complete dual rendering.

## Extension interfaces

The search evaluator boundary can later accept bitset-incremental CPU, SIMD,
WebGPU, or CUDA evaluators, provided final candidates still pass the independent
Rust decoder. The chain adapter boundary can transport tasks/results to a
future verifier. Neither GPU execution nor a Solana program is implemented or
claimed in version 1.
