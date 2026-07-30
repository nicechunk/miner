# PoUW Search v1

This document describes the original cross-profile PoUW VM search. Miner
`0.2.0-alpha.1` also includes a separate persistent NCM4 Building session; see
`ncm4-search.md`. Newly written checkpoints for both engines store complete
island populations, strategies, and deterministic RNG-generation state. The
original `CheckpointV1` reader still accepts older best-only files and resumes
them by deterministically reseeding the configured islands. The two checkpoint
magics are deliberately not interchangeable.

## Separation from consensus

The searcher is replaceable. Consensus accepts only a canonical candidate that
the independent verifier decodes to the target semantic root within limits and
that is strictly shorter than the incumbent. A seed, population, generation,
thread count, elapsed time, or claim of optimality is never trusted.

The candidate model is always:

```text
bounded typed VM program + canonical exact residual
```

The genetic engine mutates typed Rust AST nodes, not candidate bytes. After
every structural change, `exactify` decodes the structural program, computes a
complete semantic diff, and regenerates sorted SET/CLEAR/PAINT, ADD/RESTORE, or
solid ADD/CLEAR residuals. A candidate that is not exact is an internal error
and cannot enter the verified best set.

## Baselines

Every run begins with deterministic exact baselines:

- terrain: literal runs, layer bitmaps, Elias–Fano, and bounding boxes with
  exact hole restoration;
- building: material boxes, runs, extrusions, bounding structures, and literal
  fallback;
- forged items: full solid, cut boxes, axial extrusion, RLE, symmetry, sparse
  cells, and exact residual fallback.

The smallest exact encoding is the initial incumbent for search. This also
means a run can honestly finish with no improvement.

## Population and evolution

Initial populations are seeded from the deterministic baselines and typed
heuristic variants. Each island performs:

1. elite retention;
2. tournament selection;
3. profile-specific type-safe mutation;
4. same-profile subtree or spatial-region crossover;
5. exact residual regeneration and independent candidate decoding;
6. local cleanup, including empty-op removal, duplicate removal, sorted
   literals, and adjacent compatible box merging;
7. deterministic ordering by exactness, mismatch count, stored bytes,
   decode units, and finally encoding bytes.

Native builds use a bounded Rayon pool for parallel islands. Epoch boundaries
exchange only an already decoded exact global best. Browser builds use one
single-threaded Rust island per Web Worker; workers exchange only candidates
that the receiving worker verifies against its own target input.

## Reproducibility and budgets

Each island derives a ChaCha seed from the requested u64 seed and island index.
A fixed seed with one thread/island is reproducible; tests assert identical
attempt count and encoding across runs. Multi-island scheduling is intended for
throughput and should not be used as a reproducibility claim.

Configuration is bounded by population, generations, epoch generations,
attempts, wall time, memory estimate, threads, and islands. Native wall time is
checked between bounded epochs. `wasm32-unknown-unknown` has no Rust monotonic
clock; browser mining therefore runs one-generation slices and the Worker/UI
enforces the wall-clock budget between slices.

## Checkpoints

Checkpoint v1 contains:

- canonical task bytes and task ID;
- semantic root, profile, protocol/VM/search versions;
- seed and full search configuration;
- completed generation and attempt count;
- typed best program, candidate bytes, and candidate encoding hash.
- every island index, strategy, generation, RNG generation, and full typed
  population.

Resume parses the task again, verifies all identifiers and versions, and
independently rebuilds every saved population candidate before search
continues. It rejects a checkpoint from another target or config. Older v1
files without island state remain readable and restart their islands from the
recorded best and deterministic seed. CLI writes use a temporary file,
flush/sync, and atomic rename. Ctrl-C sets a cooperative stop flag; after the
current bounded epoch the best result and checkpoint are written.

## Non-goals

v1 does not claim global optimality. It includes no fake CUDA/WebGPU backend,
pool protocol, leaderboard, e-graph, CP-SAT/ILP solver, wallet, reward service,
or cross-machine coordination. Those are extension points, not simulated UI.
