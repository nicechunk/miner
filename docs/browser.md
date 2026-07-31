# Browser Miner and Privacy

The browser miner is a pure static application intended for
`https://nicechunk.com/miner/`. It uses the same `pouw-core` and `pouw-search`
Rust crates as the CLI, compiled with wasm-bindgen. JavaScript contains UI,
Worker orchestration, downloads, and plotting only; it does not reimplement the
VM, semantic root, encoding hash, or verifier.

## Execution model

- The main thread loads static configuration, accepts pasted NCM3/NCM4P/NCF1 text,
  renders the UI, and draws the lightweight gain curve.
- Each ordinary Web Worker initializes its own WASM instance and one
  single-threaded search island.
- Work runs in bounded one-generation slices so Pause/Stop/UI messages have a
  scheduling boundary.
- Worker seeds are deterministically derived from the selected seed and worker
  index.
- A best candidate is exchanged only after the receiver verifies it against
  its own target bytes.
- Stop terminates Workers instead of waiting for cooperative completion.
- Hiding the page pauses work; unloading terminates it.

For NCM4 Building runs, each Worker owns a long-lived `BrowserNcm4Session`.
Population, elite, generation, attempt count, strategy, and deterministic RNG
generation persist across slices. Verified external elites are injected into
the receiving population. Checkpoints are rate-limited into IndexedDB and bind
semantic root plus incumbent encoding hash.

SharedArrayBuffer, COOP/COEP, WebGPU, and WebGL are not required. A missing 3D
capability degrades to semantic and byte summaries without affecting mining.

## Controls and truthful metrics

The app accepts asset input only through the NCM3/NCM4P/NCF1 paste field. It opens
with the current Hollow Cottage NCM3 code in that field; repository fixtures
remain test-only and are not bundled into the public site. It never starts CPU
work automatically and defaults to at most `hardwareConcurrency - 1` Workers.
After Start, search continues without a time budget until Pause, Stop, page
visibility suspension, or unload. Start, Pause, Resume, Stop, and Reset operate
on real Workers.

All displayed byte counts, semantic roots, encoding hashes, mismatch counts,
decode units, and exact/improved status come from WASM verification. Attempts
are summed from Worker search results; elapsed time is measured by the browser
around real slices. The gain curve records actual verified best values. There
is no timer-driven fake mining rate or fabricated reduction.

Candidate bytes, canonical ResultV1, TaskV1, and a JSON verification report can
be downloaded locally. The copied CLI command verifies the binary Task/Result
pair; it does not claim a chain submission.

The Rust/WASM core detects each pasted format and selects `building`,
`forged_item`, or `terrain_delta`; JavaScript does not guess from a filename or
reimplement an NCM/NCF decoder. NCM4 input can be pasted as `NCM4P:`. Fast Analyze,
Decode, Verify, Start Deep Search, checkpoint import/export, NCM4 export, and
JSON reports all call the Rust/WASM core. The UI shows source/candidate formats,
header/body/residual/total bytes, witness and fallback state, generation,
strategy, islands, and the real best history. When NCM4 does not win it states
that the source representation remains best.

The left-hand Chunk.js asset canvas consumes canonical semantics from WASM. It
renders NCM Building voxels through the production building mesher and NCF1
component or appearance geometry through the production forge mesher. Pointer,
touch, wheel, keyboard, and reset controls provide orbit, pan, and zoom. A
missing WebGL2 context shows the canonical format, dimensions, geometry count,
and semantic root while the CPU/WASM miner remains fully usable.

The comparison panel exposes exact source/candidate semantic summaries. The
current release does not yet render two separate candidate voxel meshes with a
colored per-cell difference overlay; exact mismatch-zero verification is
complete, but that richer visualization remains follow-up work.

## Network behavior

Production runtime requests are limited to same-origin static HTML, hashed
CSS/JS/WASM/logo/locales, site configuration, and the no-cache release
manifest. Pasted NCM or NCF text never leaves the browser. There is no backend,
database, service worker, wallet, RPC, IPFS, GitHub API, third-party CDN/font,
analytics, beacon, WebSocket, or telemetry integration.

The page visibly states:

> This browser miner performs deterministic local compression and verification.
> It does not currently submit transactions or issue rewards. Your data never
> leaves this browser.

## Compatibility

The required baseline is ES modules, WebAssembly, Web Workers, BigInt, Blob
downloads, `crypto.subtle`, and Canvas 2D. The automated smoke suite
uses a real browser, runs all three profiles, asserts exact mismatch-zero
results, terminates Workers, rejects external/non-GET requests, and verifies
missing JS returns a real 404. Browsers without required APIs receive an
explicit unsupported status and retain the static documentation/download
sections.
