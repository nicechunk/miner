# NCM4 Verification and Security

## Acceptance rule

An NCM4 candidate is a valid storage improvement only when all conditions are
true after independent decoding:

```text
candidate.semanticRoot == source.semanticRoot
mismatchCount == 0
candidate.totalStoredBytes < source.totalStoredBytes
candidate.decodeUnits <= limits.maxDecodeUnits
all byte, command, material, patch, voxel, write, expansion, and memory limits hold
```

`exact` and `improved` are separate report fields. An exact but larger NCM4
record is useful as a compatibility test but is not accepted; the source NCM3
remains selected.

## Verification procedure

For direct `ncm4 verify`:

1. detect/import the source with the strict profile decoder;
2. canonicalize complete semantics and compute its semantic root once;
3. normalize binary or canonical `NCM4P:` candidate bytes;
4. validate magic, version, profile, flags, codec, varints, bit padding, palette,
   opcodes, bounds, expansion, residual canonicality, and trailing EOF;
5. execute commands in order using checked integer operations;
6. apply every exact residual action and reject overlap/no-op patches;
7. canonicalize the decoded scene and recompute root and encoding hash;
8. compare every coordinate, material, dimension, and required property;
9. recompute fixed/profile/body/residual/total bytes and decode units;
10. select NCM4 only if its normalized binary length is strictly lower.

For TaskV1/ResultV1, the existing PoUW verifier additionally reparses the task,
reimports the incumbent, recomputes task ID and both encoding hashes, and
ignores all non-consensus search metadata.

## Semantic and encoding identity

NCM4 reuses the canonical PoUW semantic model and SHA-256 domain separation:

```text
SHA256("NICECHUNK:POUW:SEMANTIC:V1\0" || profile || canonicalSemantics)
SHA256("NICECHUNK:POUW:ENCODING:V1\0" || profile || format || rawEncoding)
```

The semantic root is the asset identity. The encoding hash identifies one
representation. Tests verify that equivalent NCM3 and NCM4 have equal semantic
roots and different encoding hashes, and that changing a voxel/material or an
immutable forged property changes the semantic root.

FNV-1a-32 identifiers in current game formats remain compatibility metadata.
They are not collision-resistant and are never accepted as NCM4 semantic
identity, economic proof, or signature.

## Canonical rejection rules

The decoder rejects, rather than repairs:

- unknown magic/version/profile/codec/opcode/residual tags;
- NCM4 character records presented as NCM4 PoUW;
- padded/non-canonical Base64URL or unsigned varints;
- non-zero reserved flags, command padding, or bitmap padding;
- zero/unsorted/duplicate palette entries;
- truncated fields and any trailing bytes;
- invalid axes, zero lengths, no-op transforms, empty source regions/masks;
- coordinates or transformed destinations outside declared dimensions;
- repeat/height/crown/spacing/delta values outside fixed envelopes;
- empty or overlapping residual records and SET/CLEAR/PAINT no-ops;
- arithmetic overflow and any configured resource ceiling.

The same rules compile to native and `wasm32-unknown-unknown`; consistency tests
compare root, encoding hash, byte breakdown, decode units, and exactness.

## Limits and denial of service

The shared Rust `LimitsV1` defaults bound input bytes, commands, materials,
patches, voxels, writes, decode units, modeled memory, and per-op expansion.
NCM4 adds dimensions 1..=512, repeat count at most 512, delta magnitude at most
256, tree/fence envelopes, strict checked multiplication, and output bounds.

The parser never allocates from an unvalidated length. Encoded residual record
counts and their expanded cell counts are both bounded. Property tests feed
arbitrary bytes to NCM4, source importers, VM, Task, and Result parsers and
assert that malformed inputs return errors instead of panicking.

## Current chain boundary

Alpha 1 is a native/WASM verifier. It has not been compiled, deployed, or
benchmarked as a Solana BPF program. No Program ID, PDA migration, reward,
wallet, RPC submission, or mainnet confirmation is claimed.

The current public Backpack program at
`d70cd1b2b61e4ea8186fd0b219955f8ce64bacde` still parses only the first 108
NCF1 bits (version plus equipment header) in `verified_forge_design`, then uses
FNV-1a-32 over all supplied bytes. It does not establish that the remaining
geometry, grip, surfaces, or padding are valid NCF1. NCM4 does not silently
change that deployed contract behavior.

A future on-chain adapter should:

1. version task accounts independently from NCM3/NCM4 assets;
2. store source encoding hash, semantic root, limits, and incumbent byte cost;
3. parse canonical NCM4 with the same opcode/resource table;
4. use SHA-256 domain separation, not FNV32;
5. compare full semantics or a separately specified brick/Merkle commitment;
6. update an incumbent only when exact and strictly smaller;
7. retain an explicit rollback/upgrade authority design and full program tests.

Until that exists, a native or browser `Exact Match` is local verification, not
a chain confirmation.
