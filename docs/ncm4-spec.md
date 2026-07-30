# NCM4 PoUW Binary Specification (Alpha 1)

## Status and compatibility

This document specifies the experimental NCM4 PoUW version 1 implemented by
`pouw-core` in Miner `0.2.0-alpha.6`. NCM4 is a new format. It does not modify
the NCM3 prefix, opcodes, decoder, overwrite rules, limits, or semantic output.

Chunk.js already assigns `NCM4:` to an incompatible character-animation
record. To prevent cross-format confusion, this product-level NCM4 codec uses:

- binary magic: ASCII `NC4P`;
- text prefix: `NCM4P:` followed by canonical unpadded Base64URL;
- format ID inside PoUW task envelopes: `5` (`Ncm4PouwV1`).

`NCM4:`, `NCM4P:`, and `NCM3:` are therefore three distinct dispatch paths.

All multibyte integer work is checked. The core uses no floating point, clock,
random source, recursion, arbitrary jump, or dynamic execution.

## Canonical fixed header

Every binary record begins with exactly eight bytes:

| Offset | Size | Field | Value |
| ---: | ---: | --- | --- |
| 0 | 4 | magic | `NC4P` |
| 4 | 1 | version | `1` |
| 5 | 1 | profile | `1=terrain_delta`, `2=building`, `3=forged_item` |
| 6 | 1 | flags | must be zero |
| 7 | 1 | codec | `0=wrapped source`, `1=compact building` |

Unknown versions/codecs and non-zero reserved flags are rejected. Codec 1 is
valid only for profile 2. No trailing bytes are permitted.

## Codec 0: exact source wrapper

The wrapper provides a deterministic NCM4 dispatch and verification path while
a profile-specific compact grammar is unavailable:

```text
u8 wrappedFormat       1=ChunkBroken v1, 2=NCM3 v1, 3=NCF1 v15
u32-var payloadLength  canonical unsigned LEB128
u8[payloadLength] payload
```

The profile and wrapped format must match. The wrapped payload is parsed by the
strict existing importer, not treated as opaque bytes. The decoder recomputes
canonical semantics and the semantic root. Alpha 1 uses this codec for terrain
and forged items; because it adds 11 or 10 bytes respectively, it is not called
a compression win and deep NCM4 search is not recommended for those profiles.

## Codec 1: compact building

The byte-aligned profile header follows the fixed header:

```text
u32-var sizeX, sizeY, sizeZ     each 1..=512
u32-var paletteCount            1..=task.maxMaterials
u32-var material[paletteCount]  non-zero u16, strictly ascending
u32-var commandCount            0..=task.maxCommands
```

Unsigned varints are canonical LEB128. A redundant terminal zero group,
overflow, truncation, or value outside its field range is invalid.

The command stream is MSB-first. Define:

```text
coordBits[a] = max(1, ceil(log2(size[a])))
materialBits = max(1, ceil(log2(paletteCount)))
origin        = X, Y, Z using coordBits[X/Y/Z]
size          = X-1, Y-1, Z-1 using coordBits[X/Y/Z]
delta         = three signed values in -256..=256,
                each ZigZag encoded into exactly 10 bits
```

Every opcode is four bits. The final partial command byte must have zero
padding. Commands execute in order; later writes overwrite earlier material.
Transforms snapshot their complete source region before writing, so their own
output cannot feed back into the same operation.

## Building opcodes

| ID | Opcode | Fields after opcode | Deterministic meaning |
| ---: | --- | --- | --- |
| 0 | `BOX` | material, origin, size | Fill the bounded cuboid. |
| 1 | `REPEAT_BOX` | material, origin, size, `count-1:9`, delta | Fill 2..512 translated copies, including index 0. |
| 2 | `GABLE` | material, `style:2`, `zOriented:1`, origin, width-1, depth-1 | NCM3-compatible outline/trim/fill gable in X or Z orientation. |
| 3 | `TREE` | trunk material, leaf material, origin, `height-2:6`, `crown-1:4` | NCM3-compatible bounded trunk/crown expansion. |
| 4 | `FENCE` | material, origin, `axis:1`, length-1, `spacing-1:6` | NCM3-compatible X/Z fence with posts and rails. |
| 5 | `RUN` | material, origin, `axis:2`, length-1 | Fill one axis-aligned run. |
| 6 | `WALL` | material, origin, `normalAxis:2`, uLength-1, vLength-1, thickness-1 | Fill a plane/cuboid expressed in normal/tangent coordinates. |
| 7 | `EXTRUDE` | material, origin, `axis:2`, uLength-1, vLength-1, depth-1, `u*v` mask bits | Extrude every set mask cell by `depth`; mask must be non-empty. |
| 8 | `TRANSLATE` | source origin, source size, delta | Copy all occupied source voxels by a non-zero integer delta. |
| 9 | `ROTATE_Y` | source origin, source size, destination origin, `quarterTurns:2` | Copy with 1, 2, or 3 integer quarter-turns around Y. |
| 10 | `MIRROR` | source origin, source size, destination origin, `axis:2` | Copy and reverse local coordinates on axis 0, 1, or 2. |
| 11 | `REPEAT_REGION` | source origin, source size, `count-1:9`, delta | Snapshot once, then copy indices 1 through count-1. |
| 12 | `CLEAR_BOX` | origin, size | Remove occupied voxels; an empty/no-op clear is non-canonical. |

For GABLE, width uses the X coordinate width and depth uses the Z coordinate
width; orientation changes execution, not field allocation. Tree height is
2..=64, crown is 1..=16, fence spacing is 1..=64, repeat count is 2..=512,
and axes outside their declared range are invalid.

Every geometry or destination coordinate must remain inside the declared
building dimensions. Empty transform sources, zero deltas, no-op rotations,
zero lengths, empty extrude masks, material zero, and per-op expansion beyond
`maxExpandedPerOp` are rejected.

## Exact residual

After command padding comes one residual tag. Residual actions are:

| Action | Byte | Rule |
| --- | ---: | --- |
| `SET` | 0 | target cell must be empty; carries a palette index |
| `CLEAR` | 1 | target cell must be occupied; carries no material |
| `PAINT` | 2 | target cell must be occupied with a different material; carries a palette index |

Coordinate identity is:

```text
id = x + sizeX * (z + sizeZ * y)
```

The encoder generates the complete structural/target diff and serializes all
changes. It tries all six codecs and chooses the shortest encoding, breaking a
byte tie by residual tag. No residual may overlap itself or contain a no-op.

| Tag | Codec | Payload |
| ---: | --- | --- |
| 0 | none | no payload |
| 1 | sparse | count, gap-coded strictly increasing IDs, action/material |
| 2 | runs | run count, gap from previous end, length-1, action/material |
| 3 | boxes | count, varint origin, varint size-1, action/material; sorted by start ID |
| 4 | layers | count, Y, action/material, fixed `ceil(sizeX*sizeZ/8)` MSB-first bitmap |
| 5 | XOR bitmap | fixed `ceil(volume/8)` occupancy bitmap followed by one action/material per set bit |
| 6 | material groups | group count; canonical action/material key, count, gap-coded IDs |

All bitmap padding bits are zero. Sparse/group gaps encode the first ID
directly and later `id - previous - 1`. Layer, box, run, group, and sparse
order is canonical. Expanded patch count is bounded even when the encoded
record count is smaller.

## Cost and language audit

The stored cost is exactly the normalized binary byte length:

```text
total = fixedHeaderBytes + profileHeaderBytes + bodyBytes + residualBytes
```

The text prefix/Base64 representation is transport only and is never used for
the storage comparison. The current fixed-format lower bound reported by
preflight is 10 bytes: the eight-byte fixed header plus the minimum profile and
residual fields. It is a lower bound, not a promise that a particular semantic
scene is representable in 10 bytes.

Each written/removed/patched voxel costs one decode unit; the final count also
adds one unit per command and expanded patch. Wrapper decode units equal the
wrapped payload length. The verifier recomputes every byte count and unit from
the candidate; result metadata is never trusted.

## Canonical identity

NCM3 and NCM4 decode to the same `BuildingSemantics`: dimensions plus a sorted
coordinate/material map. Asset identity is the existing domain-separated PoUW
semantic root, not the encoding:

```text
semanticRoot = SHA256("NICECHUNK:POUW:SEMANTIC:V1\0"
                      || profile || canonicalSemantics)
encodingHash = SHA256("NICECHUNK:POUW:ENCODING:V1\0"
                      || profile || format || encodingBytes)
```

Equivalent NCM3 and NCM4 therefore have equal semantic roots and different
encoding hashes. FNV32 is not used for NCM4 identity or verification.
