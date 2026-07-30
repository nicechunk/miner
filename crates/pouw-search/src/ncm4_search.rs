use std::collections::BTreeSet;

use pouw_core::{
    decode_ncm4, deterministic_ncm4_building_program, encode_ncm4_building, encoding_hash,
    exactify_ncm4_building, semantic_root, BuildingSemantics, Error, ErrorKind, GableStyle, Hash32,
    ImportedAsset, IncumbentFormat, LimitsV1, Ncm4BuildingOp, Ncm4BuildingProgram, Ncm4Stats,
    Profile, Result, Semantics,
};
use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::{IslandStrategy, SearchConfig};

const CHECKPOINT_MAGIC: &[u8] = b"NC4S1\n";
const NCM4_SEARCH_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4SearchCandidate {
    pub program: Ncm4BuildingProgram,
    pub encoding: Vec<u8>,
    pub encoding_hash: Hash32,
    pub semantic_root: Hash32,
    pub stats: Ncm4Stats,
    pub exact: bool,
}

impl Ncm4SearchCandidate {
    fn fitness_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.stats
            .total_bytes
            .cmp(&other.stats.total_bytes)
            .then_with(|| self.stats.decode_units.cmp(&other.stats.decode_units))
            .then_with(|| self.encoding.cmp(&other.encoding))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ncm4IslandState {
    index: u16,
    generation: u32,
    rng_generation: u32,
    attempts: u64,
    strategy: IslandStrategy,
    population: Vec<Ncm4SearchCandidate>,
    seen: BTreeSet<Hash32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerializableState {
    search_version: u8,
    imported: ImportedAsset,
    semantic_root: Hash32,
    source_encoding_hash: Hash32,
    source_bytes: u32,
    config: SearchConfig,
    generation: u32,
    attempts: u64,
    best: Ncm4SearchCandidate,
    islands: Vec<Ncm4IslandState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4SearchCheckpoint {
    state: SerializableState,
}

impl Ncm4SearchCheckpoint {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let json = serde_json::to_vec(self).map_err(|error| {
            Error::new(
                ErrorKind::Internal,
                "ncm4-checkpoint-json",
                error.to_string(),
            )
        })?;
        let mut output = Vec::with_capacity(CHECKPOINT_MAGIC.len() + json.len());
        output.extend_from_slice(CHECKPOINT_MAGIC);
        output.extend_from_slice(&json);
        Ok(output)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Self> {
        if !input.starts_with(CHECKPOINT_MAGIC) {
            return Err(Error::invalid(
                "ncm4-checkpoint-magic",
                "NCM4 search checkpoint magic must be NC4S1.",
            ));
        }
        let checkpoint: Self =
            serde_json::from_slice(&input[CHECKPOINT_MAGIC.len()..]).map_err(|error| {
                Error::new(
                    ErrorKind::InvalidInput,
                    "ncm4-checkpoint-json",
                    error.to_string(),
                )
            })?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn validate(&self) -> Result<()> {
        let state = &self.state;
        if state.search_version != NCM4_SEARCH_VERSION
            || state.imported.profile != Profile::Building
            || state.source_bytes != state.imported.incumbent_encoding.len() as u32
            || encoding_hash(
                state.imported.profile,
                state.imported.format,
                &state.imported.incumbent_encoding,
            ) != state.source_encoding_hash
            || semantic_root(&state.imported.semantics) != state.semantic_root
            || state.best.semantic_root != state.semantic_root
            || !state.best.exact
            || state.islands.len() != usize::from(state.config.islands)
        {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "ncm4-checkpoint-state",
                "NCM4 checkpoint metadata is incomplete or inconsistent.",
            ));
        }
        let limits = LimitsV1::default();
        state.config.validate(&limits)?;
        state.imported.semantics.validate(&limits)?;
        let reimported = pouw_core::import_incumbent(
            state.imported.profile,
            state.imported.format,
            &state.imported.incumbent_encoding,
            &limits,
        )?;
        if reimported != state.imported.semantics {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "ncm4-checkpoint-source",
                "NCM4 checkpoint source bytes do not match its saved semantics.",
            ));
        }
        let mut island_indices = BTreeSet::new();
        for island in &state.islands {
            if island.index >= state.config.islands
                || !island_indices.insert(island.index)
                || island.population.len() != state.config.population as usize
                || island.generation != state.generation
                || island.rng_generation != island.generation
                || island.population.is_empty()
            {
                return Err(Error::invalid(
                    "ncm4-checkpoint-island",
                    "NCM4 checkpoint island state is incomplete.",
                ));
            }
        }
        Ok(())
    }

    pub fn migrate_verified_elite(&self, external: &Self) -> Result<Self> {
        self.validate()?;
        external.validate()?;
        if self.state.semantic_root != external.state.semantic_root
            || self.state.imported.semantics != external.state.imported.semantics
        {
            return Err(Error::new(
                ErrorKind::HashMismatch,
                "ncm4-checkpoint-migration-target",
                "NCM4 external elite belongs to another target.",
            ));
        }
        let limits = LimitsV1::default();
        let target = building_target(&self.state.imported)?;
        let migrant = replay_candidate(&external.state.best, target, &limits)?;
        let mut merged = self.clone();
        if migrant.fitness_cmp(&merged.state.best).is_lt() {
            merged.state.best = migrant.clone();
        }
        for island in &mut merged.state.islands {
            inject(island, &migrant);
        }
        merged.validate()?;
        Ok(merged)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4SearchProgress {
    pub generation: u32,
    pub attempts: u64,
    pub best_bytes: u32,
    pub header_bytes: u32,
    pub body_bytes: u32,
    pub residual_bytes: u32,
    pub decode_units: u64,
    pub strategy: String,
    pub semantic_root: Hash32,
    pub witness_exists: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4SearchOutcome {
    pub best: Ncm4SearchCandidate,
    pub attempts: u64,
    pub generation: u32,
    pub improved: bool,
    pub checkpoint: Ncm4SearchCheckpoint,
}

pub struct Ncm4SearchSession {
    state: SerializableState,
    target: BuildingSemantics,
    executor: Ncm4Executor,
}

impl Ncm4SearchSession {
    pub fn new(imported: ImportedAsset, config: SearchConfig) -> Result<Self> {
        let limits = LimitsV1::default();
        config.validate(&limits)?;
        let target = building_target(&imported)?.clone();
        let root = semantic_root(&imported.semantics);
        let source_encoding_hash = encoding_hash(
            imported.profile,
            imported.format,
            &imported.incumbent_encoding,
        );
        let seed_program = deterministic_ncm4_building_program(&imported, &limits)?;
        let seed = evaluate(seed_program, &target, &limits)?;
        let mut islands = Vec::with_capacity(usize::from(config.islands));
        for index in 0..config.islands {
            let global_index = u64::from(config.shard_index)
                .saturating_mul(u64::from(config.islands))
                .saturating_add(u64::from(index));
            let strategy = if global_index % 2 == 0 {
                IslandStrategy::Genetic
            } else {
                IslandStrategy::LargeNeighborhood
            };
            let population = initial_population(
                &seed,
                &target,
                &limits,
                config.population as usize,
                usize::from(index),
            );
            let seen = population
                .iter()
                .map(|candidate| candidate.encoding_hash)
                .collect();
            islands.push(Ncm4IslandState {
                index,
                generation: 0,
                rng_generation: 0,
                attempts: 0,
                strategy,
                population,
                seen,
            });
        }
        let executor = Ncm4Executor::new(&config)?;
        Ok(Self {
            state: SerializableState {
                search_version: NCM4_SEARCH_VERSION,
                source_bytes: imported.incumbent_encoding.len() as u32,
                imported,
                semantic_root: root,
                source_encoding_hash,
                config,
                generation: 0,
                attempts: 0,
                best: seed,
                islands,
            },
            target,
            executor,
        })
    }

    pub fn from_checkpoint(checkpoint: &Ncm4SearchCheckpoint) -> Result<Self> {
        checkpoint.validate()?;
        let limits = LimitsV1::default();
        let target = building_target(&checkpoint.state.imported)?.clone();
        let mut state = checkpoint.state.clone();
        state.best = replay_candidate(&state.best, &target, &limits)?;
        for island in &mut state.islands {
            let mut replayed = Vec::with_capacity(island.population.len());
            for candidate in &island.population {
                replayed.push(replay_candidate(candidate, &target, &limits)?);
            }
            replayed.sort_by(Ncm4SearchCandidate::fitness_cmp);
            island.population = replayed;
            island.seen = island
                .population
                .iter()
                .map(|candidate| candidate.encoding_hash)
                .collect();
        }
        let executor = Ncm4Executor::new(&state.config)?;
        Ok(Self {
            state,
            target,
            executor,
        })
    }

    pub fn step<F>(&mut self, generations: u32, mut progress: F) -> Result<Ncm4SearchOutcome>
    where
        F: FnMut(&Ncm4SearchProgress),
    {
        if generations == 0 {
            return Err(Error::invalid(
                "ncm4-search-generations",
                "NCM4 search step must run at least one generation.",
            ));
        }
        let limits = LimitsV1::default();
        for _ in 0..generations {
            for island in &mut self.state.islands {
                inject(island, &self.state.best);
            }
            self.executor.run(
                &mut self.state.islands,
                &self.target,
                &limits,
                &self.state.config,
                1,
            )?;
            let attempts = self
                .state
                .islands
                .iter_mut()
                .map(|island| {
                    let attempts = island.attempts;
                    island.attempts = 0;
                    attempts
                })
                .sum::<u64>();
            self.state.attempts = self.state.attempts.saturating_add(attempts);
            self.state.generation = self.state.generation.saturating_add(1);
            for island in &self.state.islands {
                if let Some(candidate) = island.population.first() {
                    if candidate.fitness_cmp(&self.state.best).is_lt() {
                        self.state.best = candidate.clone();
                    }
                }
            }
            progress(&self.progress());
        }
        self.formal_verify_best()?;
        Ok(self.outcome())
    }

    pub fn checkpoint(&self) -> Result<Ncm4SearchCheckpoint> {
        let checkpoint = Ncm4SearchCheckpoint {
            state: self.state.clone(),
        };
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn inject_checkpoint(&mut self, external: &Ncm4SearchCheckpoint) -> Result<()> {
        let merged = self.checkpoint()?.migrate_verified_elite(external)?;
        self.state = merged.state;
        Ok(())
    }

    pub fn progress(&self) -> Ncm4SearchProgress {
        let best = &self.state.best;
        Ncm4SearchProgress {
            generation: self.state.generation,
            attempts: self.state.attempts,
            best_bytes: best.stats.total_bytes,
            header_bytes: best.stats.fixed_header_bytes + best.stats.profile_header_bytes,
            body_bytes: best.stats.body_bytes,
            residual_bytes: best.stats.residual_bytes,
            decode_units: best.stats.decode_units,
            strategy: "beam-rewrite+typed-island-lns".into(),
            semantic_root: best.semantic_root,
            witness_exists: best.stats.total_bytes < self.state.source_bytes,
        }
    }

    pub fn source_bytes(&self) -> u32 {
        self.state.source_bytes
    }

    pub fn source_format(&self) -> IncumbentFormat {
        self.state.imported.format
    }

    pub fn source_encoding_hash(&self) -> Hash32 {
        self.state.source_encoding_hash
    }

    pub fn config(&self) -> &SearchConfig {
        &self.state.config
    }

    pub fn generation(&self) -> u32 {
        self.state.generation
    }

    pub fn attempts(&self) -> u64 {
        self.state.attempts
    }

    pub fn best(&self) -> &Ncm4SearchCandidate {
        &self.state.best
    }

    fn formal_verify_best(&self) -> Result<()> {
        let decoded = decode_ncm4(&self.state.best.encoding, &LimitsV1::default())?;
        if decoded.semantic_root != self.state.semantic_root
            || decoded.semantics != Semantics::Building(self.target.clone())
        {
            return Err(Error::new(
                ErrorKind::SemanticMismatch,
                "ncm4-search-final-verification",
                "NCM4 search best failed independent decode verification.",
            ));
        }
        Ok(())
    }

    fn outcome(&self) -> Ncm4SearchOutcome {
        Ncm4SearchOutcome {
            best: self.state.best.clone(),
            attempts: self.state.attempts,
            generation: self.state.generation,
            improved: self.state.best.stats.total_bytes < self.state.source_bytes,
            checkpoint: Ncm4SearchCheckpoint {
                state: self.state.clone(),
            },
        }
    }
}

#[cfg(feature = "parallel")]
struct Ncm4Executor {
    pool: rayon::ThreadPool,
}

#[cfg(feature = "parallel")]
impl Ncm4Executor {
    fn new(config: &SearchConfig) -> Result<Self> {
        Ok(Self {
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(usize::from(config.threads))
                .build()
                .map_err(|error| {
                    Error::new(ErrorKind::Internal, "ncm4-rayon-pool", error.to_string())
                })?,
        })
    }

    fn run(
        &self,
        islands: &mut [Ncm4IslandState],
        target: &BuildingSemantics,
        limits: &LimitsV1,
        config: &SearchConfig,
        generations: u32,
    ) -> Result<()> {
        use rayon::prelude::*;
        self.pool.install(|| {
            islands
                .par_iter_mut()
                .try_for_each(|island| evolve(island, target, limits, config, generations))
        })
    }
}

#[cfg(not(feature = "parallel"))]
struct Ncm4Executor;

#[cfg(not(feature = "parallel"))]
impl Ncm4Executor {
    fn new(_config: &SearchConfig) -> Result<Self> {
        Ok(Self)
    }

    fn run(
        &self,
        islands: &mut [Ncm4IslandState],
        target: &BuildingSemantics,
        limits: &LimitsV1,
        config: &SearchConfig,
        generations: u32,
    ) -> Result<()> {
        for island in islands {
            evolve(island, target, limits, config, generations)?;
        }
        Ok(())
    }
}

fn evolve(
    island: &mut Ncm4IslandState,
    target: &BuildingSemantics,
    limits: &LimitsV1,
    config: &SearchConfig,
    generations: u32,
) -> Result<()> {
    for _ in 0..generations {
        island.population.sort_by(Ncm4SearchCandidate::fitness_cmp);
        let mut next = island
            .population
            .iter()
            .take(usize::from(config.elite_count))
            .cloned()
            .collect::<Vec<_>>();
        let stream = u64::from(config.shard_index)
            .saturating_mul(u64::from(config.islands))
            .saturating_add(u64::from(island.index));
        let mut rng = generation_rng(config.seed, stream, island.generation);
        while next.len() < config.population as usize {
            let parent = tournament(&island.population, &mut rng, config.tournament_size).clone();
            let mut program =
                if island.strategy == IslandStrategy::Genetic && rng.next_u32() % 100 < 40 {
                    let other = tournament(&island.population, &mut rng, config.tournament_size);
                    crossover(&parent.program, &other.program, &mut rng)
                } else {
                    parent.program.clone()
                };
            let mutations = if island.strategy == IslandStrategy::LargeNeighborhood {
                2 + rng.next_u32() % 3
            } else {
                1
            };
            for _ in 0..mutations {
                mutate(&mut program, &mut rng);
            }
            local_rewrite(&mut program);
            island.attempts = island.attempts.saturating_add(1);
            match evaluate(program, target, limits) {
                Ok(candidate) if island.seen.insert(candidate.encoding_hash) => {
                    next.push(candidate)
                }
                _ => next.push(parent),
            }
        }
        next.sort_by(Ncm4SearchCandidate::fitness_cmp);
        island.population = next;
        island.generation = island.generation.saturating_add(1);
        island.rng_generation = island.generation;
    }
    Ok(())
}

fn evaluate(
    program: Ncm4BuildingProgram,
    target: &BuildingSemantics,
    limits: &LimitsV1,
) -> Result<Ncm4SearchCandidate> {
    let program = exactify_ncm4_building(program, target, limits)?;
    let encoding = encode_ncm4_building(&program, limits)?;
    let decoded = decode_ncm4(&encoding, limits)?;
    let root = semantic_root(&Semantics::Building(target.clone()));
    let exact =
        decoded.semantic_root == root && decoded.semantics == Semantics::Building(target.clone());
    if !exact {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "ncm4-search-candidate",
            "NCM4 candidate failed exact semantic verification.",
        ));
    }
    Ok(Ncm4SearchCandidate {
        program,
        encoding,
        encoding_hash: decoded.encoding_hash,
        semantic_root: decoded.semantic_root,
        stats: decoded.stats,
        exact,
    })
}

fn replay_candidate(
    saved: &Ncm4SearchCandidate,
    target: &BuildingSemantics,
    limits: &LimitsV1,
) -> Result<Ncm4SearchCandidate> {
    let replayed = evaluate(saved.program.clone(), target, limits)?;
    if replayed != *saved {
        return Err(Error::new(
            ErrorKind::HashMismatch,
            "ncm4-checkpoint-candidate",
            "NCM4 checkpoint candidate failed deterministic replay.",
        ));
    }
    Ok(replayed)
}

fn mutate(program: &mut Ncm4BuildingProgram, rng: &mut ChaCha8Rng) {
    if program.ops.is_empty() {
        return;
    }
    let choice = rng.next_u32() % 17;
    mutate_choice(program, rng, choice);
    program.residual = pouw_core::Ncm4Residual::None;
}

fn mutate_choice(program: &mut Ncm4BuildingProgram, rng: &mut ChaCha8Rng, choice: u32) {
    match choice {
        0 if program.ops.len() > 1 => {
            program.ops.remove(random_index(rng, program.ops.len()));
        }
        1 if program.ops.len() > 1 => {
            let left = random_index(rng, program.ops.len());
            let right = random_index(rng, program.ops.len());
            program.ops.swap(left, right);
        }
        2 => rewrite_box(program, rng),
        3 => split_box(program, rng),
        4 => combine_repeat(program),
        5 => shrink_box(program, rng),
        6 => {
            let index = random_index(rng, program.ops.len());
            if let Ncm4BuildingOp::RepeatBox { count, .. } = &mut program.ops[index] {
                if *count > 2 {
                    *count -= 1;
                }
            }
        }
        7 => rewrite_box_as_extrude(program, rng),
        8 => append_translate(program),
        9 => append_rotate(program),
        10 => append_mirror(program),
        11 => append_repeat_region(program),
        12 => append_clear(program),
        13 => append_gable(program),
        14 => append_tree(program),
        15 => append_fence(program),
        _ => synthesize_repeat_box(program),
    }
}

fn initial_population(
    seed: &Ncm4SearchCandidate,
    target: &BuildingSemantics,
    limits: &LimitsV1,
    population_size: usize,
    island_index: usize,
) -> Vec<Ncm4SearchCandidate> {
    let mut population = vec![seed.clone()];
    let mut unique = BTreeSet::from([seed.encoding_hash]);
    let mut stream = island_index as u64;
    while population.len() < population_size {
        let choice = ((population.len() + island_index) % 17) as u32;
        let mut rng = generation_rng(0x4e43_4d34, stream, choice);
        let mut program = seed.program.clone();
        mutate_choice(&mut program, &mut rng, choice);
        local_rewrite(&mut program);
        program.residual = pouw_core::Ncm4Residual::None;
        if let Ok(candidate) = evaluate(program, target, limits) {
            if unique.insert(candidate.encoding_hash) {
                population.push(candidate);
                stream = stream.saturating_add(1);
                continue;
            }
        }
        population.push(seed.clone());
        stream = stream.saturating_add(1);
    }
    population.sort_by(Ncm4SearchCandidate::fitness_cmp);
    population
}

fn rewrite_box(program: &mut Ncm4BuildingProgram, rng: &mut ChaCha8Rng) {
    let boxes = program
        .ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| matches!(op, Ncm4BuildingOp::Box { .. }).then_some(index))
        .collect::<Vec<_>>();
    if boxes.is_empty() {
        return;
    }
    let index = boxes[random_index(rng, boxes.len())];
    let Ncm4BuildingOp::Box {
        material,
        origin,
        size,
    } = program.ops[index].clone()
    else {
        return;
    };
    let extended = (0..3).filter(|axis| size[*axis] > 1).collect::<Vec<_>>();
    program.ops[index] = if extended.len() == 1 {
        Ncm4BuildingOp::Run {
            material,
            origin,
            axis: extended[0] as u8,
            length: size[extended[0]],
        }
    } else {
        let normal = (0..3).min_by_key(|axis| size[*axis]).unwrap_or(1);
        let tangent = tangent_axes(normal);
        Ncm4BuildingOp::Wall {
            material,
            origin,
            normal_axis: normal as u8,
            u_length: size[tangent[0]],
            v_length: size[tangent[1]],
            thickness: size[normal],
        }
    };
}

fn rewrite_box_as_extrude(program: &mut Ncm4BuildingProgram, rng: &mut ChaCha8Rng) {
    let boxes = program
        .ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| matches!(op, Ncm4BuildingOp::Box { .. }).then_some(index))
        .collect::<Vec<_>>();
    if boxes.is_empty() {
        return;
    }
    let index = boxes[random_index(rng, boxes.len())];
    let Ncm4BuildingOp::Box {
        material,
        origin,
        size,
    } = program.ops[index].clone()
    else {
        return;
    };
    let axis = (0..3).max_by_key(|axis| size[*axis]).unwrap_or(1);
    let tangent = tangent_axes(axis);
    let mask_len = usize::from(size[tangent[0]]) * usize::from(size[tangent[1]]);
    program.ops[index] = Ncm4BuildingOp::Extrude {
        material,
        origin,
        axis: axis as u8,
        u_length: size[tangent[0]],
        v_length: size[tangent[1]],
        depth: size[axis],
        mask: vec![true; mask_len],
    };
}

fn seed_box(program: &Ncm4BuildingProgram) -> Option<(u16, [u16; 3], [u16; 3])> {
    program.ops.iter().find_map(|op| match op {
        Ncm4BuildingOp::Box {
            material,
            origin,
            size,
        } => Some((*material, *origin, *size)),
        _ => None,
    })
}

fn adjacent_delta(program: &Ncm4BuildingProgram, origin: [u16; 3]) -> Option<[i16; 3]> {
    for axis in 0..3 {
        if origin[axis] + 1 < program.size[axis] {
            let mut delta = [0_i16; 3];
            delta[axis] = 1;
            return Some(delta);
        }
        if origin[axis] > 0 {
            let mut delta = [0_i16; 3];
            delta[axis] = -1;
            return Some(delta);
        }
    }
    None
}

fn append_translate(program: &mut Ncm4BuildingProgram) {
    let Some((_, origin, _)) = seed_box(program) else {
        return;
    };
    let Some(delta) = adjacent_delta(program, origin) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::Translate {
        source_origin: origin,
        source_size: [1, 1, 1],
        delta,
    });
}

fn append_rotate(program: &mut Ncm4BuildingProgram) {
    let Some((_, origin, _)) = seed_box(program) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::RotateY {
        source_origin: origin,
        source_size: [1, 1, 1],
        destination_origin: origin,
        quarter_turns: 1,
    });
}

fn append_mirror(program: &mut Ncm4BuildingProgram) {
    let Some((_, origin, _)) = seed_box(program) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::Mirror {
        source_origin: origin,
        source_size: [1, 1, 1],
        destination_origin: origin,
        axis: 0,
    });
}

fn append_repeat_region(program: &mut Ncm4BuildingProgram) {
    let Some((_, origin, _)) = seed_box(program) else {
        return;
    };
    let Some(delta) = adjacent_delta(program, origin) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::RepeatRegion {
        source_origin: origin,
        source_size: [1, 1, 1],
        count: 2,
        delta,
    });
}

fn append_clear(program: &mut Ncm4BuildingProgram) {
    let Some((_, origin, _)) = seed_box(program) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::ClearBox {
        origin,
        size: [1, 1, 1],
    });
}

fn append_gable(program: &mut Ncm4BuildingProgram) {
    let Some((material, _, _)) = seed_box(program) else {
        return;
    };
    let width = program.size[0]
        .min(program.size[1].saturating_mul(2))
        .max(1);
    let depth = program.size[2].clamp(1, 5);
    program.ops.push(Ncm4BuildingOp::Gable {
        material,
        origin: [0, 0, 0],
        width,
        depth,
        style: GableStyle::Fill,
        z_oriented: false,
    });
}

fn append_tree(program: &mut Ncm4BuildingProgram) {
    if program.size[0] < 4 || program.size[1] < 2 || program.size[2] < 4 {
        return;
    }
    let Some((material, _, _)) = seed_box(program) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::Tree {
        trunk_material: material,
        leaf_material: material,
        origin: [1, 0, 1],
        height: 2,
        crown: 1,
    });
}

fn append_fence(program: &mut Ncm4BuildingProgram) {
    if program.size[1] < 5 {
        return;
    }
    let Some((material, _, _)) = seed_box(program) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::Fence {
        material,
        origin: [0, 0, 0],
        length: program.size[0].clamp(1, 8),
        axis: 0,
        spacing: 1,
    });
}

fn synthesize_repeat_box(program: &mut Ncm4BuildingProgram) {
    let Some((material, origin, _)) = seed_box(program) else {
        return;
    };
    let Some(delta) = adjacent_delta(program, origin) else {
        return;
    };
    program.ops.push(Ncm4BuildingOp::RepeatBox {
        material,
        origin,
        size: [1, 1, 1],
        count: 2,
        delta,
    });
}

fn split_box(program: &mut Ncm4BuildingProgram, rng: &mut ChaCha8Rng) {
    let index = random_index(rng, program.ops.len());
    let Ncm4BuildingOp::Box {
        material,
        origin,
        size,
    } = program.ops[index].clone()
    else {
        return;
    };
    let axis = (0..3).max_by_key(|axis| size[*axis]).unwrap_or(0);
    if size[axis] < 2 {
        return;
    }
    let left_length = size[axis] / 2;
    let mut left_size = size;
    left_size[axis] = left_length;
    let mut right_origin = origin;
    right_origin[axis] += left_length;
    let mut right_size = size;
    right_size[axis] -= left_length;
    program.ops[index] = Ncm4BuildingOp::Box {
        material,
        origin,
        size: left_size,
    };
    program.ops.insert(
        index + 1,
        Ncm4BuildingOp::Box {
            material,
            origin: right_origin,
            size: right_size,
        },
    );
}

fn combine_repeat(program: &mut Ncm4BuildingProgram) {
    for left in 0..program.ops.len() {
        for right in left + 1..program.ops.len() {
            let (
                Ncm4BuildingOp::Box {
                    material: left_material,
                    origin: left_origin,
                    size: left_size,
                },
                Ncm4BuildingOp::Box {
                    material: right_material,
                    origin: right_origin,
                    size: right_size,
                },
            ) = (&program.ops[left], &program.ops[right])
            else {
                continue;
            };
            if left_material == right_material && left_size == right_size {
                let delta = [
                    i32::from(right_origin[0]) - i32::from(left_origin[0]),
                    i32::from(right_origin[1]) - i32::from(left_origin[1]),
                    i32::from(right_origin[2]) - i32::from(left_origin[2]),
                ];
                if delta.iter().all(|value| (-256..=256).contains(value)) && delta != [0, 0, 0] {
                    let replacement = Ncm4BuildingOp::RepeatBox {
                        material: *left_material,
                        origin: *left_origin,
                        size: *left_size,
                        count: 2,
                        delta: [delta[0] as i16, delta[1] as i16, delta[2] as i16],
                    };
                    program.ops[left] = replacement;
                    program.ops.remove(right);
                    return;
                }
            }
        }
    }
}

fn shrink_box(program: &mut Ncm4BuildingProgram, rng: &mut ChaCha8Rng) {
    let index = random_index(rng, program.ops.len());
    if let Ncm4BuildingOp::Box { size, .. } = &mut program.ops[index] {
        let axis = random_index(rng, 3);
        if size[axis] > 1 {
            size[axis] -= 1;
        }
    }
}

fn local_rewrite(program: &mut Ncm4BuildingProgram) {
    let mut unique = Vec::new();
    for op in program.ops.drain(..) {
        if !unique.contains(&op) {
            unique.push(op);
        }
    }
    program.ops = unique;
}

fn crossover(
    left: &Ncm4BuildingProgram,
    right: &Ncm4BuildingProgram,
    rng: &mut ChaCha8Rng,
) -> Ncm4BuildingProgram {
    let left_cut = random_index(rng, left.ops.len().saturating_add(1));
    let right_cut = random_index(rng, right.ops.len().saturating_add(1));
    let mut child = left.clone();
    child.ops = left.ops[..left_cut]
        .iter()
        .cloned()
        .chain(right.ops[right_cut..].iter().cloned())
        .collect();
    child.residual = pouw_core::Ncm4Residual::None;
    child
}

fn tournament<'a>(
    population: &'a [Ncm4SearchCandidate],
    rng: &mut ChaCha8Rng,
    size: u8,
) -> &'a Ncm4SearchCandidate {
    let mut best = random_index(rng, population.len());
    for _ in 1..size {
        let candidate = random_index(rng, population.len());
        if population[candidate].fitness_cmp(&population[best]).is_lt() {
            best = candidate;
        }
    }
    &population[best]
}

fn inject(island: &mut Ncm4IslandState, migrant: &Ncm4SearchCandidate) {
    if island
        .population
        .iter()
        .any(|candidate| candidate.encoding == migrant.encoding)
    {
        return;
    }
    if let Some(last) = island.population.last_mut() {
        *last = migrant.clone();
    }
    island.seen.insert(migrant.encoding_hash);
    island.population.sort_by(Ncm4SearchCandidate::fitness_cmp);
}

fn building_target(imported: &ImportedAsset) -> Result<&BuildingSemantics> {
    let Semantics::Building(target) = &imported.semantics else {
        return Err(Error::invalid(
            "ncm4-search-profile",
            "NCM4 alpha search currently supports the building profile.",
        ));
    };
    Ok(target)
}

fn generation_rng(seed: u64, stream: u64, generation: u32) -> ChaCha8Rng {
    let mut state = seed
        ^ stream.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ u64::from(generation).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mut bytes = [0_u8; 32];
    for chunk in bytes.chunks_exact_mut(8) {
        state = splitmix64(state);
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    ChaCha8Rng::from_seed(bytes)
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn random_index(rng: &mut ChaCha8Rng, length: usize) -> usize {
    if length <= 1 {
        0
    } else {
        (rng.next_u64() % length as u64) as usize
    }
}

fn tangent_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pouw_core::{deterministic_ncm4_seed, import_asset};

    fn fixture() -> ImportedAsset {
        import_asset(
            Profile::Building,
            include_bytes!("../../../test-vectors/building/complex-cottage.ncm3"),
            &LimitsV1::default(),
        )
        .unwrap()
    }

    fn config(generations: u32) -> SearchConfig {
        SearchConfig {
            seed: 44,
            threads: 1,
            islands: 2,
            population: 6,
            generations,
            epoch_generations: 1,
            elite_count: 1,
            tournament_size: 2,
            max_attempts: None,
            time_limit_ms: None,
            memory_limit_bytes: 32 * 1024 * 1024,
            shard_index: 0,
            shard_count: 1,
        }
    }

    #[test]
    fn ncm4_session_is_deterministic_and_keeps_population() {
        let mut first = Ncm4SearchSession::new(fixture(), config(2)).unwrap();
        let first_outcome = first.step(2, |_| {}).unwrap();
        let checkpoint_bytes = first_outcome.checkpoint.to_bytes().unwrap();
        let checkpoint = Ncm4SearchCheckpoint::from_bytes(&checkpoint_bytes).unwrap();
        let mut resumed = Ncm4SearchSession::from_checkpoint(&checkpoint).unwrap();
        let resumed_outcome = resumed.step(2, |_| {}).unwrap();

        let mut uninterrupted = Ncm4SearchSession::new(fixture(), config(4)).unwrap();
        let uninterrupted_outcome = uninterrupted.step(4, |_| {}).unwrap();
        assert_eq!(
            resumed_outcome.best.encoding,
            uninterrupted_outcome.best.encoding
        );
        assert_eq!(resumed_outcome.attempts, uninterrupted_outcome.attempts);
        assert!(resumed_outcome.improved);
    }

    #[test]
    fn ncm4_external_checkpoint_migrates_verified_elite() {
        let mut left = Ncm4SearchSession::new(fixture(), config(1)).unwrap();
        let mut right_config = config(1);
        right_config.seed = 99;
        right_config.shard_index = 1;
        right_config.shard_count = 2;
        let mut right = Ncm4SearchSession::new(fixture(), right_config).unwrap();
        let left_checkpoint = left.step(1, |_| {}).unwrap().checkpoint;
        let right_checkpoint = right.step(1, |_| {}).unwrap().checkpoint;
        let merged = left_checkpoint
            .migrate_verified_elite(&right_checkpoint)
            .unwrap();
        assert_eq!(merged.state.islands.len(), 2);
        assert!(merged.state.islands.iter().all(|island| {
            island
                .population
                .iter()
                .any(|candidate| candidate.encoding == right_checkpoint.state.best.encoding)
        }));
    }

    #[test]
    fn ncm4_checkpoint_rejects_source_and_candidate_tampering() {
        let mut session = Ncm4SearchSession::new(fixture(), config(1)).unwrap();
        let checkpoint = session.step(1, |_| {}).unwrap().checkpoint;

        let mut source_tampered = checkpoint.clone();
        source_tampered.state.source_encoding_hash[0] ^= 1;
        assert_eq!(
            Ncm4SearchSession::from_checkpoint(&source_tampered)
                .err()
                .unwrap()
                .kind,
            ErrorKind::HashMismatch
        );

        let mut candidate_tampered = checkpoint;
        candidate_tampered.state.best.encoding_hash[0] ^= 1;
        assert_eq!(
            Ncm4SearchSession::from_checkpoint(&candidate_tampered)
                .err()
                .unwrap()
                .kind,
            ErrorKind::HashMismatch
        );
    }

    #[test]
    fn ncm4_encoded_source_can_resume_search_without_negative_optimization() {
        let limits = LimitsV1::default();
        let source = fixture();
        let encoded = deterministic_ncm4_seed(&source, &limits).unwrap().encoding;
        let imported = import_asset(Profile::Building, &encoded, &limits).unwrap();
        let source_root = semantic_root(&imported.semantics);
        let source_bytes = imported.incumbent_encoding.len();
        let mut session = Ncm4SearchSession::new(imported, config(1)).unwrap();
        let outcome = session.step(1, |_| {}).unwrap();

        assert_eq!(session.source_format(), IncumbentFormat::Ncm4PouwV1);
        assert_eq!(outcome.best.semantic_root, source_root);
        assert!(outcome.best.exact);
        assert!(outcome.best.encoding.len() <= source_bytes);
    }

    #[test]
    fn typed_mutations_reach_every_generic_ncm4_instruction_family() {
        let limits = LimitsV1::default();
        let base = Ncm4BuildingProgram {
            size: [16, 16, 16],
            palette: vec![1],
            ops: vec![Ncm4BuildingOp::Box {
                material: 1,
                origin: [0, 0, 0],
                size: [4, 4, 4],
            }],
            residual: pouw_core::Ncm4Residual::None,
        };
        let Semantics::Building(target) =
            decode_ncm4(&encode_ncm4_building(&base, &limits).unwrap(), &limits)
                .unwrap()
                .semantics
        else {
            unreachable!()
        };
        for (choice, matches_family) in [
            (2, is_wall as fn(&Ncm4BuildingOp) -> bool),
            (7, is_extrude),
            (8, is_translate),
            (9, is_rotate),
            (10, is_mirror),
            (11, is_repeat_region),
            (12, is_clear),
            (13, is_gable),
            (14, is_tree),
            (15, is_fence),
            (16, is_repeat_box),
        ] {
            let mut mutated = base.clone();
            let mut rng = generation_rng(7, 0, choice);
            mutate_choice(&mut mutated, &mut rng, choice);
            assert!(mutated.ops.iter().any(matches_family), "choice {choice}");
            assert!(evaluate(mutated, &target, &limits).unwrap().exact);
        }

        let mut run = base.clone();
        run.ops[0] = Ncm4BuildingOp::Box {
            material: 1,
            origin: [0, 0, 0],
            size: [4, 1, 1],
        };
        let Semantics::Building(run_target) =
            decode_ncm4(&encode_ncm4_building(&run, &limits).unwrap(), &limits)
                .unwrap()
                .semantics
        else {
            unreachable!()
        };
        let mut rng = generation_rng(7, 1, 2);
        mutate_choice(&mut run, &mut rng, 2);
        assert!(run.ops.iter().any(is_run));
        assert!(evaluate(run, &run_target, &limits).unwrap().exact);
    }

    fn is_wall(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Wall { .. })
    }

    fn is_run(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Run { .. })
    }

    fn is_extrude(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Extrude { .. })
    }

    fn is_translate(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Translate { .. })
    }

    fn is_rotate(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::RotateY { .. })
    }

    fn is_mirror(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Mirror { .. })
    }

    fn is_repeat_region(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::RepeatRegion { .. })
    }

    fn is_clear(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::ClearBox { .. })
    }

    fn is_gable(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Gable { .. })
    }

    fn is_tree(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Tree { .. })
    }

    fn is_fence(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::Fence { .. })
    }

    fn is_repeat_box(op: &Ncm4BuildingOp) -> bool {
        matches!(op, Ncm4BuildingOp::RepeatBox { .. })
    }
}
