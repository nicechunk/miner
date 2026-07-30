# NiceChunk Proof of Useful Work Miner

This repository is the source of truth for the NiceChunk Proof of Useful Work
Miner. Version `0.2.0-alpha.1` adds the experimental NCM4 PoUW codec and a
persistent multi-island search session while preserving NCM3 byte-for-byte.
It contains the deterministic core and verifier, native CLI, WASM bindings,
static browser miner, schemas, vectors, benchmarks, and release automation.

The v1 miner searches for a shorter bounded voxel-VM program plus a canonical
exact residual. A result is useful only when independent decoding reproduces
the target semantic root exactly and the stored candidate is strictly shorter
than the incumbent encoding.

This is a research preview. It does not submit transactions, issue rewards, or
claim that the verifier is deployed as a Solana program.

## NCM4 Alpha

NCM4 PoUW is additive. Existing `NCM3:` input still enters the unchanged NCM3
decoder and produces the same canonical scene and semantic root. Both formats
meet only after decoding:

```text
NCM3 decoder ----\
                  > canonical semantic scene -> SHA-256 semantic root
NCM4 decoder ----/
```

Chunk.js already uses `NCM4:` for an incompatible character-animation record,
so this codec deliberately uses binary magic `NC4P` and text prefix `NCM4P:`.
The product remains NCM4, but an old client cannot mistake it for NCM3 or the
character record.

The Alpha building grammar has a compact palette, adaptive coordinate fields,
13 bounded opcodes, and six exact residual codecs. Language preflight reports
the complete stored-byte cost before deep search. A result wins only when an
independent decode has mismatch count zero and is strictly shorter. Otherwise
the selected representation remains NCM3.

Current measured witnesses are 57 bytes versus 64 for the real cottage, 60
versus 64 for a structural variant, and 79 versus 96 for a held-out workshop.
See [the NCM4 benchmark report](docs/ncm4-benchmarks.md) for exact roots,
parameters, and multi-thread throughput.

## Layout

- `crates/pouw-core`: NCM3/NCM4 dispatch, deterministic codecs, hashes, limits,
  and independent verification
- `crates/pouw-search`: persistent typed genetic and large-neighborhood islands
- `crates/pouw-cli`: `nicechunk-miner` native CLI
- `crates/pouw-wasm`: browser bindings to the same Rust core
- `web`: local-only static browser miner and long-lived Worker sessions
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
git -C .dependencies/chunk.js checkout 0198c1aeeadad513b6e05c75bdcbc31133d28776
git clone https://github.com/nicechunk/game.git .dependencies/game
git -C .dependencies/game checkout 58241acf2ec3c408e1af173a947e3d85753fc739
NICECHUNK_CHUNK_JS_ROOT="$PWD/.dependencies/chunk.js" \
NICECHUNK_GAME_ROOT="$PWD/.dependencies/game" npm run build:web
npm run test:compat
```

The pinned public Chunk.js checkout supplies the real terrain, character,
building, equipment, cloud, and WebGL2 rendering modules used by the static
scene. The pinned Game checkout supplies the production ChunkBroken decoder for
differential tests. GitHub Actions performs both isolated checkouts and never
silently skips compatibility testing.

## Try NCM4

```bash
# Storage-cost preflight and deterministic witness
nicechunk-miner ncm4 analyze test-vectors/building/complex-cottage.ncm3

# Export and independently verify an NCM4 candidate
nicechunk-miner ncm4 encode test-vectors/building/complex-cottage.ncm3 \
  --out cottage.nc4p
nicechunk-miner ncm4 verify \
  --source test-vectors/building/complex-cottage.ncm3 \
  --candidate cottage.nc4p

# Persistent native search; auto leaves one logical core for the OS/UI
nicechunk-miner mine test-vectors/building/complex-cottage.ncm3 \
  --threads auto --seed 123 --checkpoint cottage.nc4s.chk \
  --out cottage-best.nc4p
nicechunk-miner resume cottage.nc4s.chk --out cottage-resumed.nc4p
```

`mine` accepts `--shard-index` and `--shard-count` for deterministic,
non-overlapping search streams. Checkpoints contain the complete population,
elite, generation, attempt counter, strategy state, and reproducible RNG stream.

The release page and CLI artifacts are generated only from verified builds.
See `docs/deployment.md` before publishing or installing Nginx configuration.
The v1 VM baseline remains in `docs/benchmarks.md`; NCM4 is specified in
`docs/ncm4-spec.md` and measured in `docs/ncm4-benchmarks.md`.

## Alpha boundaries

- Compact NCM4 search currently targets Building. Terrain and forged-item NCM4
  imports are exact bounded wrappers; the existing PoUW v1 VM remains smaller
  for the terrain fixtures.
- CUDA, WebGPU, pools, wallets, rewards, and a Solana verifier are interfaces or
  roadmap work, not simulated features.
- The browser comparison exposes independently verified semantic summaries and
  exact mismatch state. A dedicated dual voxel-render/difference mesh remains a
  follow-up rendering improvement; it is not required for verification.
