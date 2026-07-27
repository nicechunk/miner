# `nicechunk-miner` CLI

## Install and verify

Use a versioned archive from an actual GitHub Release only after the static
manifest on `/miner/` marks it available. Do not use `curl | sh`. Extract the
archive and run:

```bash
nicechunk-miner --version
nicechunk-miner self-test
```

Linux/macOS checksum verification:

```bash
sha256sum -c SHA256SUMS
# macOS alternative
shasum -a 256 -c SHA256SUMS
```

Windows PowerShell verification:

```powershell
Get-FileHash .\nicechunk-miner.exe -Algorithm SHA256
Get-Content .\SHA256SUMS
```

Compare the hexadecimal values exactly. The release archive itself also has a
SHA-256 entry in the signed-off static release manifest.

## Workflow

Inspect an existing object:

```bash
nicechunk-miner inspect chunk.ncbk --profile terrain_delta
nicechunk-miner inspect asset.ncm --profile building
nicechunk-miner inspect item.ncf1 --profile forged_item
```

Create a canonical task and deterministic baseline:

```bash
nicechunk-miner task create --profile building --input asset.ncm --out task.ncpow \
  --asset-id building:example
nicechunk-miner baseline --task task.ncpow --out baseline.ncpow
```

Mine with native islands and a safe checkpoint:

```bash
nicechunk-miner mine --task task.ncpow --threads auto --time-limit 10m \
  --seed 123 --population 64 --generations 200 \
  --checkpoint state.chk --out result.ncpow
```

Resume the same task/config (the default output is `result.ncpow`):

```bash
nicechunk-miner mine --resume state.chk
```

Independently verify and decode:

```bash
nicechunk-miner verify --task task.ncpow --result result.ncpow
nicechunk-miner decode --result result.ncpow --out decoded.json
nicechunk-miner benchmark --corpus test-vectors
nicechunk-miner self-test
```

`verify` exits with validation code 4 when an encoding is exact but not
strictly smaller. This is an honest no-improvement outcome, not corruption.

## Input/output contract

Progress goes to stderr. Reports go to stdout; binary outputs go only to the
requested file. `--json` emits one compact stable JSON object. `--json-progress`
emits newline-delimited progress objects on stderr and can be combined with
`--json`.

`-` is accepted by internal readers where a path is supported, but file outputs
use atomic temporary-file rename in the destination directory. Existing task,
result, and checkpoint formats are portable between machines of different
architectures.

Reported metrics are recomputed values: attempts, attempts/s, elapsed,
program/residual/overhead/total bytes, decode units, hashes, mismatch count,
exactness, improvement, and acceptance.

## Exit codes

| Code | Meaning |
| ---: | --- |
| 0 | command completed successfully |
| 2 | invalid, unsupported, non-canonical, truncated, trailing, unknown, or out-of-range input |
| 3 | resource limit or arithmetic overflow |
| 4 | hash/semantic verification failure, or exact but not smaller |
| 70 | internal failure |

## Version string

`--version` includes the software version, source commit, protocol version, VM
version, and cost-model version. Official archives are built from a clean Git
tag and run `self-test` before upload.
