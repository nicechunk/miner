# Reproducible v1 Benchmark

Measured on 2026-07-27 with the release-mode `nicechunk-miner 0.1.0`
binary on x86_64 Linux (Intel Xeon E5-2680 v2). The command was:

```bash
nicechunk-miner --json benchmark --corpus test-vectors
```

These are deterministic exact baselines, not claims of global optimality.
`Saved` is incumbent bytes minus total candidate bytes. A negative value is
reported honestly and is not an accepted storage improvement.

| Profile | Vector | Incumbent | Program | Residual | Overhead | Candidate | Saved | Saved % | Decode units | Exact |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: |
| `terrain_delta` | normal row | 208 | 5 | 1 | 9 | 15 | 193 | 92.78% | 20 | yes |
| `terrain_delta` | complex cavity | 784 | 8 | 57 | 10 | 75 | 709 | 90.43% | 394 | yes |
| `terrain_delta` | boundary | 208 | 1 | 13 | 10 | 24 | 184 | 88.46% | 20 | yes |
| `building` | normal box | 13 | 9 | 1 | 11 | 21 | -8 | -61.53% | 264 | yes |
| `building` | complex cottage | 64 | 313 | 1 | 11 | 325 | -261 | -407.81% | 2,822 | yes |
| `building` | boundary | 31 | 21 | 1 | 13 | 35 | -4 | -12.90% | 18 | yes |
| `forged_item` | normal full | 20 | 2 | 2 | 36 | 40 | -20 | -100.00% | 1,964 | yes |
| `forged_item` | painted cavity | 32 | 9 | 11 | 43 | 63 | -31 | -96.87% | 2,198 | yes |
| `forged_item` | boundary | 35 | 6 | 2 | 48 | 56 | -21 | -60.00% | 10 | yes |

Every row was decoded again by the independent verifier with mismatch count
zero. Native and WASM produced the same semantic root, encoding hash, byte
breakdown, and decode-unit count for all nine vectors. The current JavaScript
ChunkBroken, NCM3, and NCF1 decoders produced the same canonical semantics.
