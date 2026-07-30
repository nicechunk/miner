# NCM4 Alpha Benchmark Report

## Method

Measurements were taken on 2026-07-30 UTC with
`nicechunk-miner 0.2.0-alpha.5`, Rust 1.88.0, and the NCM4 Alpha change set
based on Miner commit `f88b6773be9f5a13fa505dffd1a76092b3d28afc` for the
storage fixtures and `af54e076c1359a62efb2549bdb4939f16fe93060` for the
population-parallel comparison. The release tag/manifest records the final
immutable commit SHA. Host:

- x86_64 Linux;
- two Intel Xeon E5-2680 v2 sockets at 2.80 GHz;
- 10 physical cores/socket, two threads/core, 40 logical CPUs;
- one NUMA node reported by the host.

Deterministic storage measurements used:

```bash
cargo build --release -p pouw-cli
target/release/nicechunk-miner --json benchmark --corpus test-vectors
```

All sizes are normalized binary bytes that must be stored per asset. Base64
characters and one-time task metadata are excluded. Every NCM4 row was decoded
again and compared against complete canonical semantics. These are witnesses,
not global-optimum claims.

## Building language audit

| Fixture | Role | NCM3 bytes | NCM4 fixed | Profile | Body | Residual | NCM4 total | Saved | Saved % | Decode units | Exact | Selected |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: | --- |
| `complex-cottage` | real tuning fixture | 64 | 8 | 11 | 37 | 1 | 57 | 7 | 10.93% | 2,581 | yes | NCM4 |
| `cottage-variant` | structural variant | 64 | 8 | 11 | 40 | 1 | 60 | 4 | 6.25% | 3,614 | yes | NCM4 |
| `workshop-heldout` | held-out fixture | 96 | 8 | 11 | 59 | 1 | 79 | 17 | 17.70% | 7,672 | yes | NCM4 |
| `normal-box` | near-minimal NCM3 | 13 | 8 | 6 | 3 | 1 | 18 | -5 | -38.46% | 257 | yes | NCM3 |
| `boundary` | boundary/overlap case | 31 | 8 | 9 | 17 | 1 | 35 | -4 | -12.90% | 7 | yes | NCM3 |

The first witness generalizes to a related asset, and a held-out workshop has a
larger relative gain. The compact language contains no asset-hash lookup or
cottage-specific opcode. The two negative rows prove that selection does not
upgrade merely because NCM4 is available.

Semantic roots:

| Fixture | Root |
| --- | --- |
| `complex-cottage` | `5d08001e2f3d0d2fdd560774858c52ed2cf52fbcfd36c2111c07eafa5130e21b` |
| `cottage-variant` | `bc5039d7702f720c02d6e5cd821a4cad541ab50cbe2496c6b62fbe2f8eeb1290` |
| `workshop-heldout` | `460acc2c6fe10be340d24d5b7592586cee8f252437daf5354f7b3528df8c9d7d` |
| `normal-box` | `37c31cab8ce82ad1fdd42c4d63819eb95af309431bcba56b0d2565c6f4584c8c` |
| `boundary` | `43afefa8ff118f3a7c53fd4ff6e949cb18b52bda13b1bcda8478265ba0c1e451` |

For `complex-cottage`, search found a byte-tied 57-byte encoding with 2,580
decode units versus the deterministic seed's 2,581. It is ranked first inside
NCM4 but is not described as one additional byte of storage savings.

## Other profiles

Alpha 1 has no compact NCM4 terrain or forged grammar. Its exact wrappers are
expected to lose to their source, and preflight does not recommend deep NCM4
search:

| Profile / fixture | Source | Source bytes | NCM4 wrapper | Delta | Existing PoUW v1 VM | Exact |
| --- | --- | ---: | ---: | ---: | ---: | :---: |
| terrain / normal row | ChunkBroken v1 | 208 | 219 | -11 | 15 | yes |
| terrain / complex cavity | ChunkBroken v1 | 784 | 795 | -11 | 75 | yes |
| terrain / boundary | ChunkBroken v1 | 208 | 219 | -11 | 24 | yes |
| forged / normal full | NCF1 v15 | 20 | 30 | -10 | 40 | yes |
| forged / painted cavity | NCF1 v15 | 32 | 42 | -10 | 63 | yes |
| forged / boundary | NCF1 v15 | 35 | 45 | -10 | 56 | yes |

The wrapper establishes common dispatch and verification; it is not presented
as compression. Terrain already benefits from the existing PoUW v1 VM, while
the compact NCM4 profile work remains future language design.

## Multi-thread search throughput

Command template:

```bash
target/release/nicechunk-miner --json mine \
  test-vectors/building/complex-cottage.ncm3 \
  --threads N --islands N --population 16 --generations 20 \
  --seed 424242 --out /tmp/cottage-N.nc4p
```

The elapsed clock covers the persistent generation loop after session
construction. Population, generations, seed, target, and strategy mix were
fixed. Because each island evaluates its own population, total attempts scale
with islands and are included in the rate.

| Threads/islands | Attempts | Elapsed ms | Attempts/s | Speedup vs 1 |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 300 | 1,034 | 290.14 | 1.00x |
| 2 | 600 | 1,136 | 528.17 | 1.82x |
| 4 | 1,200 | 1,359 | 883.00 | 3.04x |
| 8 | 2,400 | 1,325 | 1,811.32 | 6.24x |
| 12 | 3,600 | 1,460 | 2,465.75 | 8.50x |

The 12-thread run created and evolved 12 islands, providing direct evidence
that the former eight-island ceiling is gone. Scaling is not linear because
candidate serialization/decoding, allocation, memory bandwidth, and the mixed
island workloads remain significant.

### Threads beyond the island count

Alpha 5 also flattens every island's offspring into one ordered Rayon batch.
This keeps 12 persistent islands while allowing a larger thread pool to evaluate
their combined population. The comparison used 40 threads, 12 islands,
population 64, 50 generations, and seed 123 on `complex-cottage`:

| Evaluator | Attempts | Elapsed ms | Mean CPU cores | Attempts/s | Relative elapsed |
| --- | ---: | ---: | ---: | ---: | ---: |
| Island-only parallelism | 36,000 | 16,592 | 8.56 | 2,169.72 | 1.00x |
| Ordered population batch | 36,000 | 8,046 | 24.57 | 4,474.27 | 0.48x |

The candidate archive and complete checkpoint had identical SHA-256 values in
both runs. A separate oversubscription check with `--threads 190 --islands 12`
created 192 process threads (190 Rayon workers plus the main and signal threads)
and completed 144,000 exact attempts in 26,898 ms on this 40-logical-CPU host.
Thread creation does not imply 190 simultaneously executing cores when the host
has fewer hardware threads.

## RTX 4090 CUDA evaluator

CUDA measurements were repeated for `0.2.0-alpha.6` on an NVIDIA GeForce RTX
4090 (24 GB, compute capability 8.9), driver 580.173.02, CUDA Toolkit 12.0, and
an Intel Xeon E5-2697A v4 host with 32 logical CPUs. The release binary does not
require the Toolkit at runtime: it dynamically loads the NVIDIA driver and
contains deterministic PTX compiled for compute capability 7.0 or newer.

Both CPU and CUDA runs used 32 threads, 12 islands, population 64, 20
generations, and seed 123. CUDA used batch size 2,048 and eight formally
evaluated survivors per island:

| Fixture | CPU attempts/s | CUDA attempts/s | CUDA/CPU | Best/source bytes | Exact |
| --- | ---: | ---: | ---: | ---: | :---: |
| `complex-cottage` | 5,891.98 | 24,448.22 | 4.15x | 57 / 64 | yes |
| `cottage-variant` | 4,068.95 | 26,325.41 | 6.47x | 60 / 64 | yes |
| `workshop-heldout` | 1,664.55 | 13,967.02 | 8.39x | 79 / 96 | yes |

For every fixture, CPU and CUDA selected the same final candidate byte length,
semantic root, and encoding hash after 14,400 attempts. The cottage hash was
`9d15665ee97236486098c97680d47399e781abd89d85933bc88fbc48b6d9a22b`.
Resuming its CUDA checkpoint advanced generation 20 to 40 and attempts 14,400
to 28,800 while retaining the same 57-byte exact candidate. The gain depends on
geometry, batch size, survivor count, driver, power limits, and CPU, so it is
not a consensus or universal performance claim.

## Upgrade comparison

Before NCM4, the generic PoUW v1 Building baseline expanded the cottage to 325
bytes (313 program + 1 residual + 11 overhead), so NCM3 correctly remained the
best 64-byte representation. NCM4's compact NCM3-preserving transcode produces
a reachable 57-byte exact witness before expensive search. The language audit
therefore satisfies the central gate: a shorter answer is known to exist before
additional CPU is spent.

## Reproduction and interpretation

Run `nicechunk-miner ncm4 analyze FILE` for the complete language audit and
`nicechunk-miner ncm4 verify --source SOURCE --candidate CANDIDATE` for an
independent comparison. Re-run the throughput table on the target deployment
hardware; attempts/s is not a consensus field and varies with CPU, power state,
memory, compiler, and background load.
