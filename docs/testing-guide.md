# Testing NCM4 Alpha

## Toolchain

- Rust `1.88.0` with `rustfmt`, `clippy`, and `wasm32-unknown-unknown`;
- Node.js 20 or newer;
- wasm-bindgen CLI `0.2.126`;
- dependencies installed with `npm ci`;
- pinned public Chunk.js and Game checkouts.

```bash
git clone https://github.com/nicechunk/chunk.js.git .dependencies/chunk.js
git -C .dependencies/chunk.js checkout 0198c1aeeadad513b6e05c75bdcbc31133d28776
git clone https://github.com/nicechunk/game.git .dependencies/game
git -C .dependencies/game checkout 58241acf2ec3c408e1af173a947e3d85753fc739
```

Set these paths for web and differential tests:

```bash
export NICECHUNK_CHUNK_JS_ROOT="$PWD/.dependencies/chunk.js"
export NICECHUNK_GAME_ROOT="$PWD/.dependencies/game"
```

The dependency directories are ignored and never belong in a commit or release
archive.

## Required local gate

Run from the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo check -p pouw-core --no-default-features
cargo build -p pouw-cli
target/debug/nicechunk-miner self-test
npm run build:web
npm run check:web
npm run test:compat
npm run test:wasm
npm run test:browser
npm run audit:secrets
git diff --check
```

To require all three Playwright engines locally:

```bash
npx playwright install --with-deps chromium firefox webkit
POUW_BROWSER_TARGETS=chromium,firefox,webkit npm run test:browser
```

Google Chrome and Microsoft Edge are also exercised when their installed
binaries are available. WebKit is the Safari-engine smoke substitute on Linux;
final Safari platform acceptance should still be repeated on macOS.

On a host with both stable browsers installed, the full isolated run is:

```bash
POUW_BROWSER_TARGETS=chrome,chromium,firefox,webkit,edge npm run test:browser
```

Multiple requested engines run in separate child processes so a closed browser
releases its WASM/WebGL memory before the next engine starts.

## What the suites prove

`cargo test --workspace` covers:

- unchanged NCM3 golden semantic roots;
- all NCM4 opcodes and six residual codecs;
- NCM4 encode/decode and NCM3/NCM4 semantic equivalence;
- text/magic collision avoidance and format auto-detection;
- unknown opcode, truncation, trailing bytes, non-canonical padding/varints,
  bounds, write/decode/memory limits, and arbitrary input non-panics;
- exact residual cell/material equality;
- deterministic one-thread search and checkpoint/resume equivalence;
- full population/RNG state restoration;
- real elite injection and a 12-thread/12-island execution path;
- CLI analyze/encode/decode/verify/mine/checkpoint integration.

`npm run test:compat` bundles a test-only export of the private production Game
`decodeChunkBrokenDeltas` function in memory, without modifying Game. It then
compares that decoder and the pinned Chunk.js NCM3/NCF1 decoders with Rust
canonical semantics for all nine original golden vectors. Missing checkouts are
a hard failure in CI.

`npm run test:wasm` compares native and WASM semantic root, encoding hash,
header/body/residual/total bytes, decode units, exactness, and mismatch count.

`npm run test:browser` serves only the local static bundle and checks long-lived
generation state, Pause/Resume/Stop, Worker termination, IndexedDB checkpoint,
verified elite exchange, NCM4 export/reimport/search, localization, responsive
layout, real 404s, same-origin GET-only traffic, and zero uploads.

## CUDA validation

CPU-only hosts still compile and test the CUDA boundary because the NVIDIA
driver is dynamically loaded. On a compatible NVIDIA host, opt into real kernel
tests explicitly:

```bash
cargo build --release -p pouw-cli --features cuda
target/release/nicechunk-miner --json gpu-info
NICECHUNK_CUDA_TEST=1 cargo test -p pouw-search --features cuda
target/release/nicechunk-miner mine test-vectors/building/complex-cottage.ncm3 \
  --accelerator cuda --threads auto --islands 12 --population 64 \
  --generations 20 --seed 123 --gpu-batch-size 2048 --gpu-survivors 8
```

The kernel parity suite covers all 13 NCM4 Building opcodes and compares GPU
mismatch, SET/CLEAR/PAINT, and patch-run results to a CPU rasterization. It also
checks CUDA checkpoint/resume against an uninterrupted fixed-seed run. Never
set `NICECHUNK_CUDA_TEST=1` on a host without a compatible NVIDIA device.

## Property and adversarial runs

The normal suite uses bounded property cases. The independent CI job increases
coverage:

```bash
PROPTEST_CASES=1024 cargo test --release \
  -p pouw-core --test protocol_adversarial
```

Long-running fuzzing should target `decode_ncm4`, each source importer,
`TaskV1::from_bytes`, `ResultV1::from_bytes`, and VM candidate decode. Corpus
findings must be reduced into deterministic regression tests before release.

## Benchmark reproduction

```bash
cargo build --release -p pouw-cli
target/release/nicechunk-miner --json benchmark --corpus test-vectors
```

For thread scaling, use the exact command and parameters in
`docs/ncm4-benchmarks.md`. Record CPU model/topology, OS, compiler, source tag,
seed, population, generations, islands, attempts, elapsed time, and candidate
bytes. Never compare rates with different attempt definitions without saying so.

## Release smoke

Each native GitHub runner executes the complete workspace tests for its target,
builds the release CLI, and runs `nicechunk-miner self-test` before packaging.
The quality job builds the Web/WASM archive after native/WASM, compatibility,
and three-engine browser tests. The publish job validates every tar/ZIP,
generates adjacent and aggregate SHA-256 files, creates a static manifest, and
marks the tag release as a pre-release.

Release tags must exactly equal the workspace/package version, for example:

```bash
git tag -a v0.2.0-alpha.6 -m "Release NCM4 Alpha 6"
git push origin v0.2.0-alpha.6
```

Do not create or advertise download links until the workflow has uploaded and
verified the corresponding assets.
