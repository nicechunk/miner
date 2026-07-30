# CUDA NCM4 Miner

## Scope and trust boundary

CUDA accelerates NCM4 Building search; it does not define the format and it is
not a consensus verifier. The GPU receives batches of legal typed AST programs,
rasterizes all 13 Building opcodes, and returns mismatch plus SET/CLEAR/PAINT
and patch-run counts. Each island keeps only its best-ranked GPU survivors.

Those survivors then use the normal Rust path:

1. regenerate the canonical exact residual;
2. serialize the complete NCM4 candidate;
3. decode it independently under protocol limits;
4. compare complete canonical semantics and the SHA-256 semantic root;
5. compare actual stored bytes with the NCM3 incumbent.

A GPU score can never promote a non-exact candidate by itself. CPU and CUDA
search may explore different amounts of work per second, but accepted encoding
bytes and verification rules are identical.

## Runtime requirements

Download the `linux-x86_64-cuda` archive from the matching GitHub pre-release.
It embeds PTX and dynamically loads `libcuda.so.1`, so the mining host needs a
working NVIDIA driver but not a CUDA Toolkit. The current PTX requires NVIDIA
compute capability 7.0 or newer. Confirm the device before mining:

```bash
./nicechunk-miner --json gpu-info
```

`cudaCompiled` confirms that the binary includes the backend. `available`
confirms that a compatible driver/device was found. Device ordinal, name,
compute capability, driver version, and visible memory are reported without
starting a search.

## Running and tuning

```bash
./nicechunk-miner mine asset.ncm3 \
  --accelerator cuda --cuda-device 0 \
  --threads 32 --islands 12 --population 64 \
  --gpu-batch-size 2048 --gpu-survivors 8 \
  --seed 123 --checkpoint state.nc4s.chk --out best.nc4p
```

Mining continues until Ctrl-C unless `--generations`, `--time-limit`, or
`--max-attempts` is supplied. `--gpu-batch-size` controls candidates per device
submission. Larger batches improve occupancy but use two dense scene buffers
per candidate; allocation is rejected before the working set exceeds 75% of
visible device memory. `--gpu-survivors` controls how many candidates per
island receive the more expensive formal CPU evaluation. Too few survivors can
reduce search quality, while too many reduce GPU speedup.

Use `--accelerator auto` to permit a reported CPU fallback when CUDA is absent.
Use `--accelerator cuda` for benchmarks and production research runs where
silent fallback would invalidate the measurement. CUDA currently supports
direct NCM3/NCM4 Building inputs only. Terrain, forged items, TaskV1 mining,
WASM, and browsers continue to use CPU.

## Reproducibility and checkpoints

Checkpoint search-state version 2 stores the active evaluator, device ordinal,
batch size, survivor count, full populations, per-island RNG generations,
attempts, and best verified candidate. Resuming a CUDA checkpoint requires a
CUDA-capable binary and compatible device. Legacy version 1 checkpoints are
migrated explicitly to the CPU evaluator.

Fixed seed and configuration reproduce checkpoint/resume state on the same
backend. CPU and CUDA prefilter schedules are not claimed to traverse identical
search paths, so compare final exact candidates rather than assuming identical
attempt order.

## RTX 4090 evidence

The Alpha 6 validation host used an RTX 4090 24 GB (compute capability 8.9),
NVIDIA driver 580.173.02, and 32 logical CPU threads. With 12 islands,
population 64, 20 generations, seed 123, batch 2,048, and eight survivors:

| Fixture | CPU attempts/s | CUDA attempts/s | Speedup | Exact best |
| --- | ---: | ---: | ---: | --- |
| complex cottage | 5,891.98 | 24,448.22 | 4.15x | 57 / 64 bytes |
| cottage variant | 4,068.95 | 26,325.41 | 6.47x | 60 / 64 bytes |
| held-out workshop | 1,664.55 | 13,967.02 | 8.39x | 79 / 96 bytes |

All rows independently decoded to the source semantic root. These results show
a material improvement on this workload, not a universal GPU multiplier; scene
volume, mutation mix, batch size, survivor count, and host CPU all matter.

## Building and kernel provenance

```bash
cargo build --release -p pouw-cli --features cuda
NICECHUNK_CUDA_TEST=1 cargo test -p pouw-search --features cuda
```

The checked-in PTX is generated from `crates/pouw-cuda/kernels/ncm4_score.cu`
by the pinned script in the same directory. Release users do not compile the
kernel at install time. CI builds the CUDA-capable Rust binary on a CPU runner,
tests its no-device fallback, packages it separately, and publishes SHA-256
sidecars plus the aggregate release manifest.
