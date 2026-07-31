# NCM4 Search Design

## Search is not consensus

The NCM4 decoder defines meaning. Search is a replaceable way to discover a
smaller byte representation. A candidate enters the best set only after it is
serialized, decoded from scratch, bounded, and shown byte-for-byte semantically
equal to the target.

The current search genome is `Ncm4BuildingProgram`, a strongly typed Rust AST.
Mutation never flips arbitrary encoded bytes. Every change clears the previous
residual; `exactify_ncm4_building` rasterizes the structural program, computes
the complete target difference, races six residual codecs, and independently
decodes the winning serialization.

## Seed and preflight

The deterministic seed transcodes real NCM3 commands into the compact NCM4
palette/bitstream. It preserves BOX, REPEAT_BOX, all gable variants, TREE, and
FENCE. A structural beam adds typed generalizations and local rewrites. The
wrapper and compact candidates race by actual binary length.

Language audit happens before search. It includes the fixed header, profile
header, body, residual, total, theoretical fixed lower bound, exactness, and
strict witness status. The CLI and page can therefore decline expensive work
when the profile has no compact search language or when the caller chooses to
keep the already shorter source.

## Persistent session state

A checkpoint contains all information required to continue the same process:

- search format/version and complete imported source;
- target semantic root and source byte count;
- seed, thread/island/population/selection budgets and shard configuration;
- global generation and attempt count;
- verified global best AST and encoding;
- every island's strategy, generation, RNG generation, complete population,
  seen encoding-hash set, and local counters.
- resolved evaluator kind, CUDA device ordinal, batch size, and formal-survivor
  count.

The binary checkpoint starts with `NC4S1\n` followed by bounded serialized
state. Search-state version 2 records the evaluator; version 1 checkpoints are
migrated explicitly to CPU. Resume validates versions, semantic root, source
encoding hash, config, population sizes, and every candidate through the
current encoder/decoder. It does not restart from the deterministic seed or
silently change a saved CUDA trajectory to CPU.

RNG is generation-addressed ChaCha8. A stream is derived from seed, shard,
island, and generation; the checkpoint records the corresponding generation.
A one-thread run with identical version/config/source is reproducible across
pause and resume. Parallel wall-clock scheduling is a throughput feature, not
a claim that completion timing is deterministic.

## Island strategies

Even-numbered global islands use typed genetic search:

1. retain the configured elite;
2. tournament-select parents;
3. perform same-type/spatial crossover for a subset of children;
4. apply one type-safe mutation;
5. run canonical local rewrites and exact residual regeneration.

Odd-numbered global islands use large-neighborhood rewrite:

1. retain the same verified elite;
2. select a parent;
3. apply two to four structural mutations/replacements;
4. simplify and exactify;
5. reinsert only a new canonical encoding hash.

Reachable mutations include BOX/RUN/WALL/EXTRUDE conversion, BOX splitting and
shrinking, repeat formation and count changes, op removal/order changes, and
bounded TRANSLATE, ROTATE_Y, MIRROR, REPEAT_REGION, CLEAR_BOX, GABLE, TREE, and
FENCE insertion. Tests force every opcode mutation path and then independently
decode the result.

At each generation boundary, the global best is injected into every island.
Browser Workers export checkpoints; a receiving Worker verifies target
identity and candidate semantics before replacing population entries. Elite
migration therefore affects subsequent search rather than only the UI.

## Threading and sharding

Native `threads=auto` resolves to `max(1, hardwareConcurrency - 1)`. The Rayon
pool is created once per session. Islands are not clamped to eight; tests build
and execute a 12-thread/12-island session. More islands increase the number of
attempts per generation, so throughput comparisons use the same population and
generation count and report total attempts divided by measured elapsed time.

Shards use:

```text
globalIsland = shardIndex * islandsPerProcess + localIsland
```

`shardIndex < shardCount` is validated. Shards can exchange final verified
candidates through ordinary files, but version 1 has no network coordinator,
task server, leaderboard, or reward protocol.

## Hot-loop status

The session caches the target semantic root, reuses its thread pool, persists
populations, and deduplicates canonical encodings. It avoids recomputing a new
population from scratch each generation. Only verified candidates migrate.

The optional CUDA backend packs typed AST operations and rasterizes complete
Building scenes in batches. It computes true SET/CLEAR/PAINT mismatch counts
and patch-run statistics for all 13 opcodes, ranks each island's offspring, and
sends only the configured survivors into the ordinary parallel Rust evaluator.
Only that CPU path can serialize, independently decode, compare semantic roots,
and promote a candidate. CUDA therefore accelerates a replaceable search stage
without changing codec bytes or verifier results.

GPU buffers and the CUDA context persist for the session. PTX is embedded in
the binary and the driver is loaded dynamically. Unsupported builds or devices
fall back only when `auto` was requested; explicit `cuda` fails closed. The
current evaluator still rasterizes whole candidates rather than dirty regions,
and WebGPU remains a separate roadmap item. See [the CUDA guide](cuda.md).

## Stopping and selection

Generation, attempt, time, population, island, thread, and modeled-memory
budgets are bounded. Ctrl-C requests cooperative stop at the next generation
boundary and atomically writes a checkpoint. Browser Pause stops scheduling
new slices; Stop terminates Worker instances.

Final selection always compares actual stored bytes. If no strict witness was
found, the report says the source format remains best. A same-size candidate
with lower decode units never masquerades as storage released.
