# NiceChunk PoUW Protocol v1

## Status and scope

Protocol v1 is a native/WASM research protocol for proving that a bounded
voxel encoding is exactly equivalent to an incumbent and strictly smaller. It
does not define rewards, wallets, mining pools, an RPC service, or a deployed
Solana verifier. `ChainAdapterV1` is only a transport boundary; v1 has no chain
adapter implementation.

The consensus input is canonical binary. JSON and Base64 are debugging and UI
representations only; their textual bytes are never hashed as a task or result.

An accepted improvement satisfies all of the following:

```text
candidate semantic root == task semantic root
mismatch count == 0
candidate byte length < incumbent byte length
decode units <= task maximum
all other task limits are satisfied
```

An exact candidate of equal or greater length is valid for inspection and
testing but is not an accepted storage improvement.

## Versions and identifiers

Protocol, VM, and cost-model versions are independent one-byte fields. v1
requires all three to equal `1`; there is no version negotiation or permissive
fallback.

Profiles use these canonical IDs:

| ID | Profile | Imported format |
| ---: | --- | --- |
| 1 | `terrain_delta` | ChunkBroken v1 |
| 2 | `building` | NCM3 v1 |
| 3 | `forged_item` | NCF1 v15 |

Incumbent formats use IDs `1=ChunkBrokenV1`, `2=Ncm3V1`, `3=Ncf1V15`, and
`4=PouwVmV1`.

All integer varints are unsigned LEB128 with a maximum of five bytes for u32
and ten bytes for u64. Redundant terminal zero groups, overflow, and truncation
are rejected. Signed VM values use ZigZag followed by canonical u32 LEB128.

## Canonical semantics

### Terrain delta

The semantic value is signed `minY` plus a strictly sorted, duplicate-free set
of local `(x, yOffset, z)` deletion coordinates. X and Z are `0..15`; Y offset
is `0..511`. There is no material-placement meaning in v1.

Canonical bytes are ZigZag `minY`, count, then gap-coded coordinate IDs where
`id = x + 16 * (z + 16 * yOffset)`.

### Building

The semantic value is a non-zero `[sizeX,sizeY,sizeZ]` and a sorted map from
coordinates to non-zero u16 materials. VM and NCM3 commands execute in order;
later writes overwrite earlier ones. Empty cells are absent. Coordinate IDs
are `x + sizeX * (z + sizeZ * y)`.

Canonical bytes contain dimensions, voxel count, gap-coded coordinate IDs, and
material varints. Command choice is not part of asset identity.

### Forged item

The semantic value includes the complete immutable equipment header and either:

- editable components: material/resource, RGB444 color, Q6 dimensions and
  offset, optional grip, complete `14×10×14` occupancy, and paint quads; or
- appearance geometry: dimensions, optional grip, and sorted material/color
  surface quads.

The 16-bit encoded volume is retained exactly as identity data. Components,
solid cell IDs, paint quads, and appearance quads are canonical and ordered.
Changing any included physical field, material, grip, occupied cell, or quad
changes canonical semantics.

## Hashes

All hashes are SHA-256 and include a distinct ASCII domain terminated by NUL:

```text
semanticRoot = SHA256("NICECHUNK:POUW:SEMANTIC:V1\0"
                      || profile_u8 || canonicalSemantics)

encodingHash = SHA256("NICECHUNK:POUW:ENCODING:V1\0"
                      || profile_u8 || format_u8 || rawEncoding)

taskId       = SHA256("NICECHUNK:POUW:TASK:V1\0" || canonicalTask)
resultId     = SHA256("NICECHUNK:POUW:RESULT:V1\0" || consensusResult)
```

`semanticRoot` identifies the final asset. `encodingHash` identifies one byte
representation. Two different exact programs therefore have the same semantic
root and different encoding hashes. FNV32 and NCM3 stable IDs are compatibility
metadata only and are never used for PoUW consensus or economic identity.

## TaskV1 (`NCPT`)

Fields are concatenated without alignment:

```text
"NCPT"
u8 protocolVersion, vmVersion, costModelVersion, profile, incumbentFormat
u8 reserved = 0
bytes assetIdUtf8
[32] semanticRoot
bytes incumbentEncoding
[32] incumbentEncodingHash
u32-var maxInputBytes, maxCommands, maxMaterials, maxPatches,
         maxVoxels, maxWrites
u64-var maxDecodeUnits, maxMemoryBytes
u32-var maxExpandedPerOp
u8 networkPresent
  if 1: bytes networkUtf8, u64-var slot, u8 expiryPresent,
        if 1: u64-var expiresAtSlot
```

`bytes` means canonical u32 length followed by exactly that many bytes. Text is
non-empty UTF-8, contains no NUL, and has a field-specific byte limit. Optional
flags must be zero or one. Trailing data is rejected. Task validation imports
the incumbent again and recomputes both hashes.

## ResultV1 (`NCPR`)

```text
"NCPR"
u8 protocolVersion
[32] taskId
bytes candidateEncoding
[32] encodingHash
u8 minerProofPresent
  if 1: bytes identity, bytes signature
u8 metadataPresent
  if 1: bytes algorithmUtf8, u64-var attempts, u64-var elapsedMs,
        u64-var seed, u32-var threads
```

Search metadata is explicitly non-consensus. `resultId` hashes the envelope
only through the optional miner proof and excludes the metadata flag and
metadata body. A verifier ignores claimed search performance and recomputes
candidate length, VM accounting, encoding hash, semantic root, and mismatch
count from bytes.

## Limits

Default task limits are 1 MiB input, 4,096 commands/materials, 131,072 patches
and voxels, 262,144 writes, 2,000,000 decode units, 64 MiB modeled memory, and
131,072 writes from one operation. Rust constants impose stricter absolute
ceilings on every task-selected value. Limits are serialized inside the task,
so native and WASM use the same values rather than UI copies.

## Verification algorithm

1. Parse canonical TaskV1 and validate versions, limits, hashes, incumbent, and
   semantic root.
2. Parse ResultV1 and validate its bounded envelope.
3. Recompute and compare task ID and candidate encoding hash.
4. Decode the candidate with the task profile and limits.
5. Re-import the incumbent, compare every semantic element, and recompute the
   candidate semantic root.
6. Recompute stored bytes and VM statistics.
7. Report `exact`, `improved`, and `accepted = exact && improved` separately.

The implementation never treats a claimed hash, byte count, cost, or client
verification status as authoritative.
