# NiceChunk PoUW / NCM4 Compatibility Audit

Audit date: 2026-07-30 UTC

## Repository authority

`nicechunk/miner` is the dedicated public source tree for the PoUW protocol,
native CLI, WASM verifier, static browser miner, vectors, and release workflow.
It contains no game wallet, deployment credential, private operations material,
or unrelated website source.

The browser scene consumes the public `nicechunk/chunk.js` runtime at the exact
revision pinned by the release workflow. The compatibility suite also consumes
the pinned public Game implementation through `NICECHUNK_GAME_ROOT`; the
generated Miner bundle remains self-contained and makes no runtime request to
GitHub.

The audited public `main` heads were:

| Repository | Commit |
| --- | --- |
| `nicechunk/game` | `58241acf2ec3c408e1af173a947e3d85753fc739` |
| `nicechunk/chunk.js` | `0198c1aeeadad513b6e05c75bdcbc31133d28776` |
| `nicechunk/nicechunk-programs` | `d70cd1b2b61e4ea8186fd0b219955f8ce64bacde` |
| `nicechunk/nicechunk-ncm-dna` | `5c3ef8e87bad656353ad527d823297dabc396231` |
| `nicechunk/nicechunk-proof-of-frontier` | `d3fcec0f2ed5efe1738b0695c59ac6358940bbb4` |

The dedicated Miner repository now exists. Release URLs remain disabled until a
real version tag has produced all verified platform archives and checksums.

## `terrain_delta` / ChunkBroken v1

The authoritative account format is defined in
`programs/nicechunk_chunk/src/state.rs` and mirrored in
`sdk/nicechunk-chunk.ts` and `play/play-chain-chunk-deltas.js`.

- Magic is `NCBK`, version is `1`, and the fixed header is 16 bytes.
- Each deletion is one 24-bit little-endian record.
- Bits `0..3` are local X, bits `4..7` are local Z, and bits `8..16` are a
  9-bit Y offset from the account's signed `min_y`.
- X and Z are each `0..15`; Y offset is `0..511`.
- Initial capacity is 64 records, growth is 64, and maximum capacity is 2,048.
- Account length is exactly `16 + capacity * 3`, so unused capacity is paid
  storage even though only `count` records are semantic.
- Current append order is historical operation order. Membership checks prevent
  normal duplicate mining, but the state representation is not sorted and the
  low-level append helper itself does not canonicalize.
- The production game client decodes every record into an air-block delta. It
  has no arbitrary material-placement semantic in ChunkBroken v1.

PoUW imports only the first `count` records, deduplicates and sorts their bounded
coordinate identities, and preserves `min_y`. It deliberately does not add a
placement meaning to this profile.

Risk: the SDK decoder reads the version but does not reject an unsupported
version, while the play decoder does. The main game adapter also has a lenient
path that returns an empty delta list for malformed data. PoUW uses strict
version, size, range, and trailing-data checks.

## `building` / NCM3

The public `chunk.js` implementation at the audited head is stricter than the
currently modified main-worktree copy, so the public bounded decoder is the
compatibility reference for import validation.

- Prefix is `NCM3:` and binary format version is `1`.
- Dimensions are unsigned canonical varints, each `1..256`.
- Maximum payload is 65,535 bytes, command count is 4,096, expanded operations
  are 262,144, and final voxels are 131,072.
- Current opcodes are `BOX=1`, `REPEAT_BOX=2`, `GABLE=3`, `TREE=4`, `FENCE=5`,
  `GABLE_TRIM=6`, `GABLE_FILL=7`, `GABLE_Z=8`, `GABLE_TRIM_Z=9`, and
  `GABLE_FILL_Z=10`.
- Unsigned varints are limited to five bytes and reject a redundant final zero.
  Signed repeat offsets use odd/even zig-zag mapping over signed 32-bit values.
- The public decoder requires canonical unpadded Base64URL and rejects trailing
  bytes, unknown opcodes, truncation, invalid materials, and out-of-bounds
  expansion.
- Commands expand to cuboids and later writes overwrite earlier writes. Final
  canonical semantics are therefore a sorted coordinate-to-material map, not a
  command list or a visual mesh.
- Building placement applies integer translation and quarter-turn rotation only;
  one NCM3 voxel remains one world voxel.

PoUW imports all current NCM3 opcodes exactly. NCM4 adds a separate bounded
compact grammar and exact residual without changing NCM3 itself. The current
NCM3 fixtures retain their pre-upgrade semantic roots in unit, native/WASM, and
production-JavaScript differential tests.

Risk: NCM3's `stableCodeId` is FNV-1a over text and is a cache/display
identifier, not an economic asset identity.

## NCM4 name collision and dispatch

The current Chunk.js repository already uses the text prefix `NCM4:` for a
character-animation format. Reusing those bytes for a voxel codec would create
ambiguous dispatch and could let one client parse another product's data. Miner
therefore uses `NC4P` binary magic and `NCM4P:` text while publishing the
feature as NCM4 PoUW. NCM3 still uses only `NCM3:`. See `ncm4-spec.md` for the
versioned layout.

NCM4's current compact codec is Building-specific. Terrain and forged assets can
be imported/verified through an exact wrapper but are not claimed to compress;
preflight does not recommend deep NCM4 search for those profiles.

## `forged_item` / NCF1

The current public format is NCF1 version 15 with a 640-byte chain limit.
It is an MSB-first bitstream with zero padding limited to the final partial byte.

- The immutable equipment header contains 16-bit mass in 5 g units, a 16-bit
  base-16 volume encoding, and twelve 6-bit attributes.
- Component mode supports 1..31 editable components. Each component includes a
  resource/material ID, RGB444 color, Q6 dimensions and offset, optional grip
  offset/normal/quarter-turn, a complete `14 x 10 x 14` solid occupancy grid,
  and canonical painted surface quads.
- Solid occupancy uses the shortest deterministic choice among full solid,
  non-overlapping cut boxes, axial extruded-mask RLE, and full RLE.
- Appearance mode stores Q6 dimensions, optional grip, and 1..4,095 sorted,
  non-overlapping material/color surface quads on a `24^3` grid, with an
  optional coordinate palette.
- Client canonicalization validates non-empty components, ranges, surfaces,
  resource IDs, padding, and complete geometry when `decodeNcf1` is used.

The Backpack verifier at the audited public programs commit still does **not**
perform that geometry validation.
`verified_forge_design` reads only the first 108 bits (version plus equipment
header), derives material requirements, and returns FNV-1a-32 over all supplied
bytes. It accepts any 14..640 byte payload whose header is plausible, including
payloads whose remaining geometry is malformed. `ForgedItemAccount::validate`
reuses that header-only verifier and checks the same 32-bit hash.

Consequences:

1. FNV32 is collision-prone and must not be reused as the PoUW semantic root.
2. Current chain acceptance binds raw bytes but does not prove decodable NCF1
   geometry, canonical padding, grip validity, or surface validity.
3. Different exact encodings of the same geometry receive different current
   design hashes.

PoUW v1 strictly decodes complete NCF1 geometry and includes material, grip, the
full solid/quad geometry, and immutable equipment fields in its SHA-256 semantic
root. It does not alter Program IDs, PDA seeds, or existing forge economics.

## NCM-DNA and Proof of Frontier

Current NCM-DNA genes are a fixed humanoid/avatar parameter record. Mutation
increments or decrements one scalar and sometimes flips ornament flags. The
worker compares generated cuboid voxel sets and calls a match exact when a
64-bit FNV-1a hash of a canonical JSON-like cuboid string matches. Its
evolutionary mode has elite retention but no typed subtree crossover, automatic
exact residual, independent consensus verifier, or storage-byte VM fitness.
PoUW therefore treats NCM-DNA as prior search work, not as a consensus
codec or reusable verifier.

Proof of Frontier is a separate system. It combines frontier contour
seed compression with an ordinary nonce hash prefix. It is not a deployed PoUW
task/result protocol and is not wired into this miner.

## Website and Nginx

The Miner build emits one static `web/dist` tree with content-hashed JS, CSS,
WASM, locale, image, and configuration assets. It does not publish built-in
test vectors; browser asset input is paste-only. NiceChunk's complete static-site
build publishes that output at `/miner/`; it never publishes the Rust source
tree, test tools, dependency checkout, or build cache.

The production route serves existing `/miner/` files, but its current generic
fallback still returns homepage HTML with HTTP 200 for missing JavaScript and
WASM paths. The browser refuses to execute that response because its MIME is
`text/html`, yet the route still needs the reviewed administrator-installed
static location to return a strict 404. The standalone Nginx fixture tests the
intended MIME, cache, CSP, compression, atomic release switching, and rollback
behavior without becoming a browser backend.

## Domain separation

PoUW v1 uses separate SHA-256 domains for canonical semantics, candidate bytes,
tasks, and results. FNV identifiers remain compatibility metadata only. The v1
implementation is native/WASM compression software and does not claim Solana BPF
compatibility or on-chain result acceptance.
