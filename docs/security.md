# Security Model

## Trust boundary

Tasks, incumbents, results, checkpoints, local browser files, and network-
obtained bytes are untrusted. Only the Rust verifier decides exactness and
resource use. Search output, JavaScript UI state, Result metadata, claimed
hashes, and timing counters are not authority.

The verifier validates the Task, re-imports the incumbent, recomputes its
encoding hash and semantic root, recomputes the task ID, validates the Result,
recomputes its encoding hash, executes the bounded VM, compares complete
semantics, and recomputes all costs.

## Parser and VM hardening

- canonical bounded varints only;
- exact magic and version checks;
- no ignored suffix or trailing executable data;
- checked addition, multiplication, offsets, and coordinate transforms;
- command, material, patch, voxel, write, decode-unit, byte, expansion, and
  modeled-memory ceilings sourced from Rust constants;
- no float, recursion, arbitrary branches, dynamic execution, environment,
  clock, file, network, or random access in `pouw-core`;
- canonical ordering and duplicate/no-op rejection for sparse data and patches;
- bitset and NCF1 padding must be zero;
- final semantic validation after execution.

Property tests feed arbitrary short byte strings to all candidate/Profile,
Task, Result, and incumbent parsers. Adversarial tests cover non-canonical
varints, unknown opcodes, truncation, trailing data, oversized expansion,
tampered task IDs, roots, encoding hashes, cost versions, and semantically
different candidates. These tests reduce risk; they are not a substitute for
an independent security audit.

## Hashing

PoUW uses domain-separated SHA-256. Existing FNV32 design hashes are not
collision resistant and must not be treated as an asset identity, semantic
root, signature, or proof. Existing NCM stable IDs are likewise cache/display
identifiers only.

No digital signature scheme is prescribed in v1. `MinerProofV1` is an opaque,
bounded optional identity/signature envelope for a future adapter. A consumer
must define and verify its own signature domain before trusting it.

## Current forged-item chain risk

The audited Backpack program reads the NCF1 version and 104-bit equipment
payload after it, but does not parse the remaining geometry before accepting a
forge design. It hashes supplied bytes with FNV-1a-32. Therefore current chain
acceptance does not establish that geometry, grip, paint, padding, or surface
data is valid NCF1, and collision resistance is weak.

PoUW does not silently change that deployed behavior. Its forged profile
strictly parses complete NCF1 v15 and includes complete geometry and immutable
properties in the SHA-256 semantic root. Bridging this verifier to a chain
requires a separately reviewed protocol upgrade; native/WASM success is not a
Solana confirmation.

## Browser privacy

The production page is static. User-selected files are read with browser File
APIs and transferred only to same-origin Workers. The app contains no fetch,
XHR, beacon, WebSocket, wallet, RPC, IPFS, GitHub API, third-party CDN,
telemetry, or analytics path for those bytes. Built-in assets are same-origin
GETs. The browser smoke test rejects external requests and all non-GET traffic.

Browser extensions, a compromised origin, a modified binary, or a compromised
operating system remain outside this guarantee. Users verifying valuable data
should compare release SHA-256 values and use the native verifier on a second
machine.

## Availability and denial of service

Limits prevent unbounded VM expansion, but mining is intentionally CPU-heavy.
The browser never starts automatically, defaults to leaving one logical core
free, pauses when hidden, and terminates Workers on Stop. Nginx applies static
asset behavior only and runs no mining backend.

## Secrets and release hygiene

No private key, token, wallet, `.env`, employee author list, checkpoint, or
task/result working file belongs in the public miner tree. Release CI uses the
GitHub-provided token only through standard release actions and embeds no
credential in artifacts. `scripts/audit-secrets.mjs` scans tracked release
inputs before publication; any match blocks release pending manual review.

Please report vulnerabilities privately to the NiceChunk maintainers before
opening a public issue containing exploit details or secrets.
