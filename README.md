# NiceChunk Proof of Useful Work Miner

This directory is the isolated source of truth for the NiceChunk Proof of
Useful Work Miner v1. It contains the consensus codec and verifier, native
search engine and CLI, WASM bindings, static browser miner, protocol schemas,
test vectors, release automation, and deployment documentation.

The v1 miner searches for a shorter bounded voxel-VM program plus a canonical
exact residual. A result is useful only when independent decoding reproduces
the target semantic root exactly and the stored candidate is strictly shorter
than the incumbent encoding.

This is a research preview. It does not submit transactions, issue rewards, or
claim that the verifier is deployed as a Solana program.

## Layout

- `crates/pouw-core`: deterministic formats, importers, VM, hashes, and verifier
- `crates/pouw-search`: typed genetic search and deterministic baselines
- `crates/pouw-cli`: `nicechunk-miner` native CLI
- `crates/pouw-wasm`: browser bindings to the same Rust core
- `web`: local-only static browser miner
- `schemas`: Task/Result debug JSON Schemas
- `test-vectors`: cross-runtime golden vectors and corpus
- `docs`: protocol, compatibility, security, and deployment notes
- `nginx`: reviewed `/miner/` configuration and offline deployment tests

## Development

```bash
cargo test --workspace
cargo run -p pouw-cli -- self-test
npm ci
git clone https://github.com/nicechunk/chunk.js.git .dependencies/chunk.js
git -C .dependencies/chunk.js checkout f3113bf0e376b3ccca59fe773d3995bb18e656ee
NICECHUNK_CHUNK_JS_ROOT="$PWD/.dependencies/chunk.js" node scripts/build-web.mjs
```

The pinned public Chunk.js checkout supplies the real terrain, character,
building, equipment, cloud, and WebGL2 rendering modules used by the static
scene. GitHub Actions performs the same isolated checkout automatically.

The release page and CLI artifacts are generated only from verified builds.
See `docs/deployment.md` before publishing or installing Nginx configuration.
The reproducible nine-vector results are recorded in `docs/benchmarks.md`.
