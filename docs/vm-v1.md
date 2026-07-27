# NiceChunk Bounded Voxel VM and Cost Model v1

## Envelope and execution rules

A candidate starts with:

```text
"NCPV" | vmVersion=1 | profile | costModelVersion=1 | reserved=0
```

The profile body follows immediately. Every integer is deterministic; there is
no floating point, recursion, arbitrary jump, dynamic code, system time, or
unbounded loop. Commands execute in byte order. Arithmetic is checked. Unknown
opcodes, non-shortest varints, non-zero reserved/padding bits, no-op residuals,
truncation, and trailing bytes are errors.

`decodeUnits` is `baseUnits + writes` for each command or patch. The tables
below list `baseUnits`; some data-dependent parsing work is included in the
base. Bytes and decode units are independent metrics. Stored-byte reduction is
the only v1 acceptance improvement; decode units break ties for search ranking
but never become a storage claim.

## Terrain profile

The body starts with ZigZag i16 `minY` and a command count. Commands build a
deletion set. A sorted residual then performs exact `ADD` or `RESTORE` changes.

| Opcode | Operation | Base units |
| ---: | --- | ---: |
| `0x01` | `DELETE_RUN(start,length)` | 4 |
| `0x02` | `DELETE_BOX(x,y,z,w,h,d)` | 8 |
| `0x03` | `LAYER_BITMAP(y,32 bytes)` | 44 |
| `0x04` | canonical Elias–Fano sorted set | `20 + highBitmapBits` |
| `0x80` | residual `ADD(id)` | 4 |
| `0x81` | residual `RESTORE(id)` | 4 |

Run length and box volume are charged as writes. Elias–Fano fixes its low-bit
width from the v1 universe and element count; alternate widths, bad padding,
incorrect set-bit count, unsorted values, and out-of-range values are rejected.

## Building profile

The body starts with three dimensions and a command count. Materials are
non-zero u16 values. Later writes overwrite earlier writes; CUT clears. Literal
voxels and residuals use sorted, gap-coded coordinate IDs.

| Opcode | Operation | Base units |
| ---: | --- | ---: |
| `0x01` | `BOX(material,origin,size)` | 8 |
| `0x02` | `RUN(material,origin,axis,length)` | 5 |
| `0x03` | `WALL(material,origin,normal,u,v,thickness)` | 7 |
| `0x04` | `EXTRUDE(material,origin,axis,u,v,depth,mask)` | `12 + maskBits` |
| `0x05` | bounded `REPEAT(material,box,count,delta)` | `12 + count` |
| `0x06` | `MIRROR(sourceBox,axis,pivotTwice)` | `16 + copiedWrites` |
| `0x07` | `CUT(origin,size)` | 7 |
| `0x08` | sorted `LITERAL(voxels)` | `8 + count` |
| `0x80` | residual `SET(empty,material)` | 5 |
| `0x81` | residual `CLEAR(occupied)` | 5 |
| `0x82` | residual `PAINT(occupied,newMaterial)` | 5 |

Axes are `0..2`. Mirror uses the pure integer rule
`target[axis] = pivotTwice - source[axis]`. Repeat deltas are ZigZag i32 and
the count is at most 512. Every generated coordinate must remain inside the
declared dimensions.

## Forged-item profile

The body begins with little-endian u16 mass, little-endian u16 encoded volume,
twelve six-bit-range attribute bytes, and a geometry mode.

Component mode contains 1..31 component headers followed by solid commands,
exact solid patches, and paint quads. The solid grid is always `14×10×14`.

| Opcode | Operation | Base units |
| ---: | --- | ---: |
| `0x01` | `SOLID` | 4 |
| `0x02` | `CUT_BOX(origin,size)` | 7 |
| `0x03` | axial profile `EXTRUDE(axis,mask)` | `10 + maskBits` |
| `0x04` | full-grid `RLE` | `8 + runCount` |
| `0x05` | integer `SYMMETRY(axis)` | `8 + sourceCount` |
| `0x06` | sorted `SPARSE(cellIds)` | `6 + count` |
| `0x80` | residual `ADD(cell)` | 4 |
| `0x81` | residual `CLEAR(cell)` | 4 |

Paint quads add base 10 plus one write. Appearance quads add base 12 plus one
write. Quad ranges, RGB444, material IDs, order, overlap, grip axis/sign/
rotation, and final non-empty geometry are all validated.

## Resource accounting

The decoder tracks encoded program bytes, encoded residual bytes, fixed
overhead, command count, patch count, writes, final voxels, modeled memory, and
decode units. Checks occur while expanding, not only after allocation. A final
global check recomputes total byte accounting and rejects all task-limit
violations.

The cost model is versioned because any future opcode charge change would alter
candidate ordering and verification. VM v1 and cost model v1 are immutable.
