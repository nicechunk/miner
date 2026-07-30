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
const NCM4_SEARCH_VERSION: u8 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ncm4EvaluatorKind {
    #[default]
    Cpu,
    Auto,
    Cuda,
}

impl Ncm4EvaluatorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Auto => "auto",
            Self::Cuda => "cuda",
        }
    }
}

impl core::str::FromStr for Ncm4EvaluatorKind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "cpu" => Ok(Self::Cpu),
            "auto" => Ok(Self::Auto),
            "cuda" | "gpu" => Ok(Self::Cuda),
            _ => Err(Error::invalid(
                "ncm4-evaluator",
                "NCM4 evaluator must be auto, cpu, or cuda.",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4EvaluatorConfig {
    pub kind: Ncm4EvaluatorKind,
    pub cuda_device: u16,
    pub gpu_batch_size: u32,
    pub gpu_survivors_per_island: u32,
}

impl Default for Ncm4EvaluatorConfig {
    fn default() -> Self {
        Self {
            kind: Ncm4EvaluatorKind::Cpu,
            cuda_device: 0,
            gpu_batch_size: 2_048,
            gpu_survivors_per_island: 8,
        }
    }
}

impl Ncm4EvaluatorConfig {
    fn validate(&self, search: &SearchConfig) -> Result<()> {
        let _ = search;
        if self.gpu_batch_size == 0
            || self.gpu_batch_size > 65_535
            || self.gpu_survivors_per_island == 0
            || self.gpu_survivors_per_island > 16_384
        {
            return Err(Error::limit(
                "ncm4-evaluator-config",
                "NCM4 GPU batch or survivor configuration is outside its bounded envelope.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4CudaDeviceInfo {
    pub ordinal: u32,
    pub name: String,
    pub compute_major: i32,
    pub compute_minor: i32,
    pub total_memory_bytes: u64,
    pub driver_version: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ncm4EvaluatorInfo {
    pub requested: Ncm4EvaluatorKind,
    pub active: Ncm4EvaluatorKind,
    pub cuda_compiled: bool,
    pub device: Option<Ncm4CudaDeviceInfo>,
    pub fallback_reason: Option<String>,
}

pub const fn cuda_compiled() -> bool {
    cfg!(feature = "cuda")
}

pub fn cuda_devices() -> Result<Vec<Ncm4CudaDeviceInfo>> {
    #[cfg(feature = "cuda")]
    {
        pouw_cuda::devices()
            .map(|devices| devices.into_iter().map(cuda_device_info).collect())
            .map_err(|error| Error::new(ErrorKind::Internal, "ncm4-cuda-probe", error.to_string()))
    }
    #[cfg(not(feature = "cuda"))]
    {
        Err(Error::invalid(
            "ncm4-cuda-not-compiled",
            "This nicechunk-miner build does not include CUDA support.",
        ))
    }
}

#[cfg(feature = "cuda")]
fn cuda_device_info(device: pouw_cuda::CudaDeviceInfo) -> Ncm4CudaDeviceInfo {
    Ncm4CudaDeviceInfo {
        ordinal: device.ordinal,
        name: device.name,
        compute_major: device.compute_major,
        compute_minor: device.compute_minor,
        total_memory_bytes: device.total_memory_bytes as u64,
        driver_version: device.driver_version,
    }
}

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
    #[serde(default)]
    evaluator: Ncm4EvaluatorConfig,
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
        let mut checkpoint: Self = serde_json::from_slice(&input[CHECKPOINT_MAGIC.len()..])
            .map_err(|error| {
                Error::new(
                    ErrorKind::InvalidInput,
                    "ncm4-checkpoint-json",
                    error.to_string(),
                )
            })?;
        if checkpoint.state.search_version == 1 {
            checkpoint.state.search_version = NCM4_SEARCH_VERSION;
            checkpoint.state.evaluator = Ncm4EvaluatorConfig::default();
        }
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
        state.evaluator.validate(&state.config)?;
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
                || island
                    .population
                    .iter()
                    .any(|candidate| !island.seen.contains(&candidate.encoding_hash))
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
    pub evaluator: String,
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
        Self::new_with_evaluator(imported, config, Ncm4EvaluatorConfig::default())
    }

    pub fn new_with_evaluator(
        imported: ImportedAsset,
        config: SearchConfig,
        evaluator: Ncm4EvaluatorConfig,
    ) -> Result<Self> {
        let limits = LimitsV1::default();
        config.validate(&limits)?;
        evaluator.validate(&config)?;
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
        let executor = Ncm4Executor::new(&config, &target, &limits, &evaluator)?;
        let resolved_evaluator = executor.resolved_config();
        Ok(Self {
            state: SerializableState {
                search_version: NCM4_SEARCH_VERSION,
                source_bytes: imported.incumbent_encoding.len() as u32,
                imported,
                semantic_root: root,
                source_encoding_hash,
                config,
                evaluator: resolved_evaluator,
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
        }
        let executor = Ncm4Executor::new(&state.config, &target, &limits, &state.evaluator)?;
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
            evaluator: self.executor.info().active.as_str().into(),
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

    pub fn evaluator_config(&self) -> &Ncm4EvaluatorConfig {
        &self.state.evaluator
    }

    pub fn evaluator_info(&self) -> &Ncm4EvaluatorInfo {
        self.executor.info()
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
    backend: Ncm4ExecutorBackend,
    info: Ncm4EvaluatorInfo,
    resolved_config: Ncm4EvaluatorConfig,
    #[cfg(test)]
    probe: Option<std::sync::Arc<ParallelEvaluationProbe>>,
}

#[cfg(feature = "parallel")]
enum Ncm4ExecutorBackend {
    Cpu,
    #[cfg(feature = "cuda")]
    Cuda(Box<CudaBatchEvaluator>),
}

#[cfg(feature = "parallel")]
impl Ncm4Executor {
    fn new(
        config: &SearchConfig,
        target: &BuildingSemantics,
        limits: &LimitsV1,
        evaluator: &Ncm4EvaluatorConfig,
    ) -> Result<Self> {
        let (backend, info) = create_parallel_backend(target, limits, evaluator)?;
        let mut resolved_config = evaluator.clone();
        resolved_config.kind = info.active;
        Ok(Self {
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(usize::from(config.threads))
                .build()
                .map_err(|error| {
                    Error::new(ErrorKind::Internal, "ncm4-rayon-pool", error.to_string())
                })?,
            backend,
            info,
            resolved_config,
            #[cfg(test)]
            probe: None,
        })
    }

    fn resolved_config(&self) -> Ncm4EvaluatorConfig {
        self.resolved_config.clone()
    }

    fn info(&self) -> &Ncm4EvaluatorInfo {
        &self.info
    }

    fn run(
        &mut self,
        islands: &mut [Ncm4IslandState],
        target: &BuildingSemantics,
        limits: &LimitsV1,
        config: &SearchConfig,
        generations: u32,
    ) -> Result<()> {
        use rayon::prelude::*;
        for _ in 0..generations {
            let (prepared, work) = prepare_generation_batch(islands, config);
            let evaluated = match &mut self.backend {
                Ncm4ExecutorBackend::Cpu => self.pool.install(|| {
                    work.into_par_iter()
                        .map(|work| {
                            #[cfg(test)]
                            let _probe_guard = self.probe.as_ref().map(|probe| probe.enter());
                            evaluate_work(work, target, limits)
                        })
                        .collect()
                }),
                #[cfg(feature = "cuda")]
                Ncm4ExecutorBackend::Cuda(cuda) => evaluate_cuda_batch(
                    &self.pool,
                    cuda,
                    work,
                    target,
                    limits,
                    config,
                    #[cfg(test)]
                    self.probe.as_ref(),
                )?,
            };
            commit_generation_batch(islands, prepared, evaluated)?;
        }
        Ok(())
    }
}

#[cfg(all(feature = "parallel", feature = "cuda"))]
fn create_parallel_backend(
    target: &BuildingSemantics,
    limits: &LimitsV1,
    evaluator: &Ncm4EvaluatorConfig,
) -> Result<(Ncm4ExecutorBackend, Ncm4EvaluatorInfo)> {
    let requested = evaluator.kind;
    if requested == Ncm4EvaluatorKind::Cpu {
        return Ok((
            Ncm4ExecutorBackend::Cpu,
            Ncm4EvaluatorInfo {
                requested,
                active: Ncm4EvaluatorKind::Cpu,
                cuda_compiled: true,
                device: None,
                fallback_reason: None,
            },
        ));
    }
    match CudaBatchEvaluator::new(target, limits, evaluator) {
        Ok(cuda) => {
            let device = Some(cuda.device.clone());
            Ok((
                Ncm4ExecutorBackend::Cuda(Box::new(cuda)),
                Ncm4EvaluatorInfo {
                    requested,
                    active: Ncm4EvaluatorKind::Cuda,
                    cuda_compiled: true,
                    device,
                    fallback_reason: None,
                },
            ))
        }
        Err(error) if requested == Ncm4EvaluatorKind::Auto => Ok((
            Ncm4ExecutorBackend::Cpu,
            Ncm4EvaluatorInfo {
                requested,
                active: Ncm4EvaluatorKind::Cpu,
                cuda_compiled: true,
                device: None,
                fallback_reason: Some(error.message),
            },
        )),
        Err(error) => Err(error),
    }
}

#[cfg(all(feature = "parallel", not(feature = "cuda")))]
fn create_parallel_backend(
    _target: &BuildingSemantics,
    _limits: &LimitsV1,
    evaluator: &Ncm4EvaluatorConfig,
) -> Result<(Ncm4ExecutorBackend, Ncm4EvaluatorInfo)> {
    if evaluator.kind == Ncm4EvaluatorKind::Cuda {
        return Err(Error::invalid(
            "ncm4-cuda-not-compiled",
            "This nicechunk-miner build does not include CUDA support.",
        ));
    }
    Ok((
        Ncm4ExecutorBackend::Cpu,
        Ncm4EvaluatorInfo {
            requested: evaluator.kind,
            active: Ncm4EvaluatorKind::Cpu,
            cuda_compiled: false,
            device: None,
            fallback_reason: (evaluator.kind == Ncm4EvaluatorKind::Auto)
                .then(|| "CUDA support is not compiled into this binary.".into()),
        },
    ))
}

#[cfg(not(feature = "parallel"))]
struct Ncm4Executor {
    info: Ncm4EvaluatorInfo,
    resolved_config: Ncm4EvaluatorConfig,
}

#[cfg(not(feature = "parallel"))]
impl Ncm4Executor {
    fn new(
        _config: &SearchConfig,
        _target: &BuildingSemantics,
        _limits: &LimitsV1,
        evaluator: &Ncm4EvaluatorConfig,
    ) -> Result<Self> {
        if evaluator.kind == Ncm4EvaluatorKind::Cuda {
            return Err(Error::invalid(
                "ncm4-cuda-not-compiled",
                "CUDA requires a native parallel nicechunk-miner build.",
            ));
        }
        let mut resolved_config = evaluator.clone();
        resolved_config.kind = Ncm4EvaluatorKind::Cpu;
        Ok(Self {
            info: Ncm4EvaluatorInfo {
                requested: evaluator.kind,
                active: Ncm4EvaluatorKind::Cpu,
                cuda_compiled: false,
                device: None,
                fallback_reason: (evaluator.kind == Ncm4EvaluatorKind::Auto)
                    .then(|| "CUDA is unavailable in this build.".into()),
            },
            resolved_config,
        })
    }

    fn resolved_config(&self) -> Ncm4EvaluatorConfig {
        self.resolved_config.clone()
    }

    fn info(&self) -> &Ncm4EvaluatorInfo {
        &self.info
    }

    fn run(
        &mut self,
        islands: &mut [Ncm4IslandState],
        target: &BuildingSemantics,
        limits: &LimitsV1,
        config: &SearchConfig,
        generations: u32,
    ) -> Result<()> {
        for _ in 0..generations {
            let (prepared, work) = prepare_generation_batch(islands, config);
            let evaluated = work
                .into_iter()
                .map(|work| evaluate_work(work, target, limits))
                .collect();
            commit_generation_batch(islands, prepared, evaluated)?;
        }
        Ok(())
    }
}

struct PreparedIslandGeneration {
    next: Vec<Ncm4SearchCandidate>,
    offspring_count: usize,
}

struct OffspringWork {
    program: Ncm4BuildingProgram,
    fallback: Ncm4SearchCandidate,
}

struct EvaluatedOffspring {
    candidate: Option<Result<Ncm4SearchCandidate>>,
    fallback: Ncm4SearchCandidate,
}

#[cfg(feature = "cuda")]
struct CudaBatchEvaluator {
    scorer: pouw_cuda::CudaScorer,
    device: Ncm4CudaDeviceInfo,
    batch_size: usize,
    survivors_per_island: usize,
    volume: u32,
}

#[cfg(feature = "cuda")]
impl CudaBatchEvaluator {
    fn new(
        target: &BuildingSemantics,
        limits: &LimitsV1,
        config: &Ncm4EvaluatorConfig,
    ) -> Result<Self> {
        let volume = target
            .size
            .iter()
            .try_fold(1_u32, |value, dimension| {
                value.checked_mul(u32::from(*dimension))
            })
            .ok_or_else(|| Error::overflow("NCM4 CUDA target volume overflow."))?;
        let mut dense_target = vec![0_u16; volume as usize];
        for voxel in &target.voxels {
            dense_target[pouw_core::building_coord_id(target.size, *voxel) as usize] =
                voxel.material;
        }
        let scorer = pouw_cuda::CudaScorer::new(
            u32::from(config.cuda_device),
            target.size,
            &dense_target,
            limits.max_expanded_per_op,
            limits.max_writes,
        )
        .map_err(|error| Error::new(ErrorKind::Internal, "ncm4-cuda-init", error.to_string()))?;
        let device = cuda_device_info(scorer.device().clone());
        Ok(Self {
            scorer,
            device,
            batch_size: config.gpu_batch_size as usize,
            survivors_per_island: config.gpu_survivors_per_island as usize,
            volume,
        })
    }

    fn scores(&mut self, work: &[OffspringWork]) -> Result<Vec<pouw_cuda::PackedScore>> {
        let mut output = Vec::with_capacity(work.len());
        for chunk in work.chunks(self.batch_size) {
            let mut operations = Vec::new();
            let mut offsets = Vec::with_capacity(chunk.len() + 1);
            let mut masks = Vec::new();
            offsets.push(0);
            for candidate in chunk {
                pack_cuda_program(&candidate.program, &mut operations, &mut masks)?;
                offsets.push(u32::try_from(operations.len()).map_err(|_| {
                    Error::limit(
                        "ncm4-cuda-operation-count",
                        "NCM4 CUDA operation batch exceeds u32.",
                    )
                })?);
            }
            let scores = self
                .scorer
                .score(&operations, &offsets, &masks)
                .map_err(|error| {
                    Error::new(ErrorKind::Internal, "ncm4-cuda-score", error.to_string())
                })?;
            output.extend(scores);
        }
        Ok(output)
    }
}

#[cfg(all(feature = "parallel", feature = "cuda"))]
fn evaluate_cuda_batch(
    pool: &rayon::ThreadPool,
    cuda: &mut CudaBatchEvaluator,
    work: Vec<OffspringWork>,
    target: &BuildingSemantics,
    limits: &LimitsV1,
    config: &SearchConfig,
    #[cfg(test)] probe: Option<&std::sync::Arc<ParallelEvaluationProbe>>,
) -> Result<Vec<EvaluatedOffspring>> {
    use rayon::prelude::*;

    let scores = cuda.scores(&work)?;
    if scores.len() != work.len() {
        return Err(Error::new(
            ErrorKind::Internal,
            "ncm4-cuda-result-count",
            "NCM4 CUDA evaluator returned an incomplete score batch.",
        ));
    }
    let offspring_per_island = config
        .population
        .saturating_sub(u32::from(config.elite_count)) as usize;
    let mut selected = vec![false; work.len()];
    for start in (0..work.len()).step_by(offspring_per_island) {
        let end = (start + offspring_per_island).min(work.len());
        let mut ranking = (start..end).collect::<Vec<_>>();
        ranking.sort_by_key(|index| cuda_rank(&scores[*index], &work[*index], cuda.volume, *index));
        for index in ranking
            .into_iter()
            .take(cuda.survivors_per_island.min(end - start))
        {
            selected[index] = true;
        }
    }
    Ok(pool.install(|| {
        work.into_par_iter()
            .enumerate()
            .map(|(index, work)| {
                let candidate = selected[index].then(|| {
                    #[cfg(test)]
                    let _probe_guard = probe.map(|probe| probe.enter());
                    evaluate(work.program, target, limits)
                });
                EvaluatedOffspring {
                    candidate,
                    fallback: work.fallback,
                }
            })
            .collect()
    }))
}

#[cfg(feature = "cuda")]
fn cuda_rank(
    score: &pouw_cuda::PackedScore,
    work: &OffspringWork,
    volume: u32,
    original_index: usize,
) -> (u8, u64, u32, usize, usize) {
    let sparse_estimate = u64::from(score.mismatches)
        .saturating_mul(3)
        .saturating_add(u64::from(score.set_patches + score.paint_patches));
    let run_estimate = u64::from(score.patch_runs).saturating_mul(6);
    let bitmap_estimate = u64::from(volume.div_ceil(8))
        .saturating_add(u64::from(score.paint_patches).saturating_mul(2));
    let residual_estimate = sparse_estimate.min(run_estimate).min(bitmap_estimate);
    let structural_estimate = (work.program.ops.len() as u64)
        .saturating_mul(6)
        .saturating_add(
            work.program
                .ops
                .iter()
                .map(|op| match op {
                    Ncm4BuildingOp::Extrude { mask, .. } => mask.len().div_ceil(8) as u64,
                    _ => 0,
                })
                .sum::<u64>(),
        );
    (
        u8::from(score.valid == 0),
        structural_estimate.saturating_add(residual_estimate),
        score.mismatches,
        work.program.ops.len(),
        original_index,
    )
}

#[cfg(feature = "cuda")]
fn pack_cuda_program(
    program: &Ncm4BuildingProgram,
    output: &mut Vec<pouw_cuda::PackedOp>,
    masks: &mut Vec<u8>,
) -> Result<()> {
    use pouw_cuda::{PackedOp, PackedOpKind};

    for operation in &program.ops {
        let mut packed = match operation {
            Ncm4BuildingOp::Box { .. } => PackedOp::new(PackedOpKind::Box),
            Ncm4BuildingOp::RepeatBox { .. } => PackedOp::new(PackedOpKind::RepeatBox),
            Ncm4BuildingOp::Gable { .. } => PackedOp::new(PackedOpKind::Gable),
            Ncm4BuildingOp::Tree { .. } => PackedOp::new(PackedOpKind::Tree),
            Ncm4BuildingOp::Fence { .. } => PackedOp::new(PackedOpKind::Fence),
            Ncm4BuildingOp::Run { .. } => PackedOp::new(PackedOpKind::Run),
            Ncm4BuildingOp::Wall { .. } => PackedOp::new(PackedOpKind::Wall),
            Ncm4BuildingOp::Extrude { .. } => PackedOp::new(PackedOpKind::Extrude),
            Ncm4BuildingOp::Translate { .. } => PackedOp::new(PackedOpKind::Translate),
            Ncm4BuildingOp::RotateY { .. } => PackedOp::new(PackedOpKind::RotateY),
            Ncm4BuildingOp::Mirror { .. } => PackedOp::new(PackedOpKind::Mirror),
            Ncm4BuildingOp::RepeatRegion { .. } => PackedOp::new(PackedOpKind::RepeatRegion),
            Ncm4BuildingOp::ClearBox { .. } => PackedOp::new(PackedOpKind::ClearBox),
        };
        let p = &mut packed.parameters;
        match operation {
            Ncm4BuildingOp::Box {
                material,
                origin,
                size,
            } => {
                p[0] = i32::from(*material);
                set_origin_and_size(p, *origin, *size);
            }
            Ncm4BuildingOp::RepeatBox {
                material,
                origin,
                size,
                count,
                delta,
            } => {
                p[0] = i32::from(*material);
                set_origin_and_size(p, *origin, *size);
                p[8] = i32::from(*count);
                set_delta(p, *delta);
            }
            Ncm4BuildingOp::Gable {
                material,
                origin,
                width,
                depth,
                style,
                z_oriented,
            } => {
                p[0] = i32::from(*material);
                set_origin(p, *origin);
                p[5] = i32::from(*width);
                p[6] = i32::from(*depth);
                p[8] = match style {
                    GableStyle::Outline => 0,
                    GableStyle::Trim => 1,
                    GableStyle::Fill => 2,
                };
                p[9] = i32::from(*z_oriented);
            }
            Ncm4BuildingOp::Tree {
                trunk_material,
                leaf_material,
                origin,
                height,
                crown,
            } => {
                p[0] = i32::from(*trunk_material);
                p[1] = i32::from(*leaf_material);
                set_origin(p, *origin);
                p[5] = i32::from(*height);
                p[6] = i32::from(*crown);
            }
            Ncm4BuildingOp::Fence {
                material,
                origin,
                length,
                axis,
                spacing,
            } => {
                p[0] = i32::from(*material);
                set_origin(p, *origin);
                p[5] = i32::from(*length);
                p[8] = i32::from(*axis);
                p[9] = i32::from(*spacing);
            }
            Ncm4BuildingOp::Run {
                material,
                origin,
                axis,
                length,
            } => {
                p[0] = i32::from(*material);
                set_origin(p, *origin);
                p[5] = i32::from(*length);
                p[8] = i32::from(*axis);
            }
            Ncm4BuildingOp::Wall {
                material,
                origin,
                normal_axis,
                u_length,
                v_length,
                thickness,
            } => {
                p[0] = i32::from(*material);
                set_origin(p, *origin);
                p[5] = i32::from(*u_length);
                p[6] = i32::from(*v_length);
                p[7] = i32::from(*thickness);
                p[8] = i32::from(*normal_axis);
            }
            Ncm4BuildingOp::Extrude {
                material,
                origin,
                axis,
                u_length,
                v_length,
                depth,
                mask,
            } => {
                p[0] = i32::from(*material);
                set_origin(p, *origin);
                p[5] = i32::from(*u_length);
                p[6] = i32::from(*v_length);
                p[7] = i32::from(*depth);
                p[8] = i32::from(*axis);
                p[16] = i32::try_from(masks.len()).map_err(|_| {
                    Error::limit("ncm4-cuda-mask", "NCM4 CUDA mask batch exceeds i32.")
                })?;
                masks.extend(mask.iter().map(|value| u8::from(*value)));
            }
            Ncm4BuildingOp::Translate {
                source_origin,
                source_size,
                delta,
            } => {
                set_origin_and_size(p, *source_origin, *source_size);
                set_delta(p, *delta);
            }
            Ncm4BuildingOp::RotateY {
                source_origin,
                source_size,
                destination_origin,
                quarter_turns,
            } => {
                set_origin_and_size(p, *source_origin, *source_size);
                set_destination(p, *destination_origin);
                p[15] = i32::from(*quarter_turns);
            }
            Ncm4BuildingOp::Mirror {
                source_origin,
                source_size,
                destination_origin,
                axis,
            } => {
                set_origin_and_size(p, *source_origin, *source_size);
                set_destination(p, *destination_origin);
                p[15] = i32::from(*axis);
            }
            Ncm4BuildingOp::RepeatRegion {
                source_origin,
                source_size,
                count,
                delta,
            } => {
                set_origin_and_size(p, *source_origin, *source_size);
                p[8] = i32::from(*count);
                set_delta(p, *delta);
            }
            Ncm4BuildingOp::ClearBox { origin, size } => {
                set_origin_and_size(p, *origin, *size);
            }
        }
        output.push(packed);
    }
    Ok(())
}

#[cfg(feature = "cuda")]
fn set_origin(parameters: &mut [i32; 20], origin: [u16; 3]) {
    parameters[2..5].copy_from_slice(&origin.map(i32::from));
}

#[cfg(feature = "cuda")]
fn set_origin_and_size(parameters: &mut [i32; 20], origin: [u16; 3], size: [u16; 3]) {
    set_origin(parameters, origin);
    parameters[5..8].copy_from_slice(&size.map(i32::from));
}

#[cfg(feature = "cuda")]
fn set_delta(parameters: &mut [i32; 20], delta: [i16; 3]) {
    parameters[9..12].copy_from_slice(&delta.map(i32::from));
}

#[cfg(feature = "cuda")]
fn set_destination(parameters: &mut [i32; 20], destination: [u16; 3]) {
    parameters[12..15].copy_from_slice(&destination.map(i32::from));
}

fn prepare_generation_batch(
    islands: &mut [Ncm4IslandState],
    config: &SearchConfig,
) -> (Vec<PreparedIslandGeneration>, Vec<OffspringWork>) {
    let mut prepared = Vec::with_capacity(islands.len());
    let offspring_per_island = config
        .population
        .saturating_sub(u32::from(config.elite_count)) as usize;
    let mut work = Vec::with_capacity(offspring_per_island.saturating_mul(islands.len()));

    for island in islands {
        island.population.sort_by(Ncm4SearchCandidate::fitness_cmp);
        let next = island
            .population
            .iter()
            .take(usize::from(config.elite_count))
            .cloned()
            .collect::<Vec<_>>();
        let stream = u64::from(config.shard_index)
            .saturating_mul(u64::from(config.islands))
            .saturating_add(u64::from(island.index));
        let mut rng = generation_rng(config.seed, stream, island.generation);
        let work_start = work.len();
        while work.len() - work_start < offspring_per_island {
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
            work.push(OffspringWork {
                program,
                fallback: parent,
            });
        }

        prepared.push(PreparedIslandGeneration {
            next,
            offspring_count: offspring_per_island,
        });
    }

    (prepared, work)
}

fn evaluate_work(
    work: OffspringWork,
    target: &BuildingSemantics,
    limits: &LimitsV1,
) -> EvaluatedOffspring {
    EvaluatedOffspring {
        candidate: Some(evaluate(work.program, target, limits)),
        fallback: work.fallback,
    }
}

fn commit_generation_batch(
    islands: &mut [Ncm4IslandState],
    prepared: Vec<PreparedIslandGeneration>,
    evaluated: Vec<EvaluatedOffspring>,
) -> Result<()> {
    if islands.len() != prepared.len()
        || evaluated.len()
            != prepared
                .iter()
                .map(|island| island.offspring_count)
                .sum::<usize>()
    {
        return Err(Error::new(
            ErrorKind::Internal,
            "ncm4-evaluation-batch",
            "NCM4 parallel evaluation batch is incomplete.",
        ));
    }

    // Indexed parallel iteration preserves this order. Committing in the same
    // order as the old serial loop keeps seen-set and fallback behavior stable.
    let mut evaluated = evaluated.into_iter();
    for (island, mut prepared) in islands.iter_mut().zip(prepared) {
        for _ in 0..prepared.offspring_count {
            let Some(result) = evaluated.next() else {
                return Err(Error::new(
                    ErrorKind::Internal,
                    "ncm4-evaluation-order",
                    "NCM4 evaluation results ended before the generation was committed.",
                ));
            };
            match result.candidate {
                Some(Ok(candidate)) if island.seen.insert(candidate.encoding_hash) => {
                    prepared.next.push(candidate)
                }
                _ => prepared.next.push(result.fallback),
            }
        }
        prepared.next.sort_by(Ncm4SearchCandidate::fitness_cmp);
        island.population = prepared.next;
        island.generation = island.generation.saturating_add(1);
        island.rng_generation = island.generation;
    }
    Ok(())
}

#[cfg(all(test, feature = "parallel"))]
struct ParallelEvaluationProbe {
    active: std::sync::atomic::AtomicUsize,
    max_active: std::sync::atomic::AtomicUsize,
    workers: std::sync::Mutex<BTreeSet<usize>>,
    delay: std::time::Duration,
}

#[cfg(all(test, feature = "parallel"))]
impl ParallelEvaluationProbe {
    fn new(delay: std::time::Duration) -> Self {
        Self {
            active: std::sync::atomic::AtomicUsize::new(0),
            max_active: std::sync::atomic::AtomicUsize::new(0),
            workers: std::sync::Mutex::new(BTreeSet::new()),
            delay,
        }
    }

    fn enter(&self) -> ParallelEvaluationGuard<'_> {
        use std::sync::atomic::Ordering;

        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let mut observed = self.max_active.load(Ordering::SeqCst);
        while active > observed {
            match self.max_active.compare_exchange_weak(
                observed,
                active,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => observed = current,
            }
        }
        if let Some(index) = rayon::current_thread_index() {
            self.workers.lock().unwrap().insert(index);
        }
        std::thread::sleep(self.delay);
        ParallelEvaluationGuard { probe: self }
    }

    fn max_active(&self) -> usize {
        self.max_active.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn worker_count(&self) -> usize {
        self.workers.lock().unwrap().len()
    }
}

#[cfg(all(test, feature = "parallel"))]
struct ParallelEvaluationGuard<'a> {
    probe: &'a ParallelEvaluationProbe,
}

#[cfg(all(test, feature = "parallel"))]
impl Drop for ParallelEvaluationGuard<'_> {
    fn drop(&mut self) {
        self.probe
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
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

    #[cfg(feature = "parallel")]
    #[test]
    fn population_batch_uses_more_workers_than_islands() {
        let mut search_config = config(1);
        search_config.threads = 8;
        search_config.islands = 2;
        search_config.population = 65;
        search_config.memory_limit_bytes = 128 * 1024 * 1024;
        let mut session = Ncm4SearchSession::new(fixture(), search_config).unwrap();
        let probe = std::sync::Arc::new(ParallelEvaluationProbe::new(
            std::time::Duration::from_millis(2),
        ));
        session.executor.probe = Some(probe.clone());

        session.step(1, |_| {}).unwrap();

        assert!(probe.max_active() > 2, "max active={}", probe.max_active());
        assert!(
            probe.worker_count() > 2,
            "worker count={}",
            probe.worker_count()
        );
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_batch_preserves_fixed_seed_trajectory() {
        let mut single_config = config(3);
        single_config.population = 12;
        let mut parallel_config = single_config.clone();
        parallel_config.threads = 8;

        let mut single = Ncm4SearchSession::new(fixture(), single_config).unwrap();
        let mut parallel = Ncm4SearchSession::new(fixture(), parallel_config).unwrap();
        let single_outcome = single.step(3, |_| {}).unwrap();
        let parallel_outcome = parallel.step(3, |_| {}).unwrap();

        assert_eq!(single_outcome.best, parallel_outcome.best);
        assert_eq!(single_outcome.attempts, parallel_outcome.attempts);
        assert_eq!(single.state.islands, parallel.state.islands);
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

    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_prefilter_matches_cpu_scene_differences() {
        if std::env::var_os("NICECHUNK_CUDA_TEST").is_none() {
            return;
        }
        let limits = LimitsV1::default();
        let seed_program = Ncm4BuildingProgram {
            size: [16, 16, 16],
            palette: vec![1],
            ops: vec![Ncm4BuildingOp::Box {
                material: 1,
                origin: [0, 0, 0],
                size: [4, 4, 4],
            }],
            residual: pouw_core::Ncm4Residual::None,
        };
        let Semantics::Building(target) = decode_ncm4(
            &encode_ncm4_building(&seed_program, &limits).unwrap(),
            &limits,
        )
        .unwrap()
        .semantics
        else {
            unreachable!()
        };
        let seed = evaluate(seed_program.clone(), &target, &limits).unwrap();
        let mut programs = vec![seed_program.clone()];
        for choice in [2_u32, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16] {
            let mut program = seed_program.clone();
            let mut rng = generation_rng(7, 0, choice);
            mutate_choice(&mut program, &mut rng, choice);
            program.residual = pouw_core::Ncm4Residual::None;
            cpu_prefilter_score(&program, &target, &limits).unwrap();
            programs.push(program);
        }
        let mut run = seed_program.clone();
        run.ops[0] = Ncm4BuildingOp::Box {
            material: 1,
            origin: [0, 0, 0],
            size: [4, 1, 1],
        };
        let mut rng = generation_rng(7, 1, 2);
        mutate_choice(&mut run, &mut rng, 2);
        run.residual = pouw_core::Ncm4Residual::None;
        programs.push(run);
        for family in [
            is_wall as fn(&Ncm4BuildingOp) -> bool,
            is_run,
            is_extrude,
            is_translate,
            is_rotate,
            is_mirror,
            is_repeat_region,
            is_clear,
            is_gable,
            is_tree,
            is_fence,
            is_repeat_box,
        ] {
            assert!(programs.iter().flat_map(|program| &program.ops).any(family));
        }
        let config = Ncm4EvaluatorConfig {
            kind: Ncm4EvaluatorKind::Cuda,
            cuda_device: 0,
            gpu_batch_size: 128,
            gpu_survivors_per_island: 4,
        };
        let mut cuda = CudaBatchEvaluator::new(&target, &limits, &config).unwrap();
        let work = programs
            .iter()
            .cloned()
            .map(|program| OffspringWork {
                program,
                fallback: seed.clone(),
            })
            .collect::<Vec<_>>();
        let gpu = cuda.scores(&work).unwrap();
        for (index, program) in programs.iter().enumerate() {
            let cpu = cpu_prefilter_score(program, &target, &limits).unwrap();
            assert_eq!(gpu[index].valid, 1, "program {index}");
            assert_eq!(gpu[index].mismatches, cpu.mismatches, "program {index}");
            assert_eq!(gpu[index].set_patches, cpu.set_patches, "program {index}");
            assert_eq!(
                gpu[index].clear_patches, cpu.clear_patches,
                "program {index}"
            );
            assert_eq!(
                gpu[index].paint_patches, cpu.paint_patches,
                "program {index}"
            );
            assert_eq!(gpu[index].patch_runs, cpu.patch_runs, "program {index}");
        }
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_checkpoint_resume_reproduces_search_state() {
        if std::env::var_os("NICECHUNK_CUDA_TEST").is_none() {
            return;
        }
        let evaluator = Ncm4EvaluatorConfig {
            kind: Ncm4EvaluatorKind::Cuda,
            cuda_device: 0,
            gpu_batch_size: 128,
            gpu_survivors_per_island: 4,
        };
        let mut search_config = config(2);
        search_config.threads = 4;
        search_config.population = 12;
        let checkpoint = {
            let mut first = Ncm4SearchSession::new_with_evaluator(
                fixture(),
                search_config.clone(),
                evaluator.clone(),
            )
            .unwrap();
            first.step(1, |_| {}).unwrap().checkpoint
        };
        let resumed = {
            let mut session = Ncm4SearchSession::from_checkpoint(&checkpoint).unwrap();
            session.step(1, |_| {}).unwrap()
        };
        let uninterrupted = {
            let mut session =
                Ncm4SearchSession::new_with_evaluator(fixture(), search_config, evaluator).unwrap();
            session.step(2, |_| {}).unwrap()
        };
        assert_eq!(resumed.best, uninterrupted.best);
        assert_eq!(resumed.attempts, uninterrupted.attempts);
        assert_eq!(resumed.checkpoint.state, uninterrupted.checkpoint.state);
    }

    #[cfg(feature = "cuda")]
    fn cpu_prefilter_score(
        program: &Ncm4BuildingProgram,
        target: &BuildingSemantics,
        limits: &LimitsV1,
    ) -> Result<pouw_cuda::PackedScore> {
        let mut structural = program.clone();
        structural.residual = pouw_core::Ncm4Residual::None;
        let decoded = decode_ncm4(&encode_ncm4_building(&structural, limits)?, limits)?;
        let Semantics::Building(base) = decoded.semantics else {
            unreachable!()
        };
        let volume = target
            .size
            .iter()
            .fold(1_usize, |value, dimension| value * usize::from(*dimension));
        let mut before = vec![0_u16; volume];
        let mut after = vec![0_u16; volume];
        for voxel in base.voxels {
            before[pouw_core::building_coord_id(base.size, voxel) as usize] = voxel.material;
        }
        for voxel in &target.voxels {
            after[pouw_core::building_coord_id(target.size, *voxel) as usize] = voxel.material;
        }
        let mut score = pouw_cuda::PackedScore {
            valid: 1,
            ..pouw_cuda::PackedScore::default()
        };
        let mut previous = None;
        for (before, after) in before.into_iter().zip(after) {
            let signature = match (before, after) {
                (left, right) if left == right => None,
                (0, material) => {
                    score.set_patches += 1;
                    Some((1_u8, material))
                }
                (_, 0) => {
                    score.clear_patches += 1;
                    Some((2_u8, 0))
                }
                (_, material) => {
                    score.paint_patches += 1;
                    Some((3_u8, material))
                }
            };
            if let Some(signature) = signature {
                score.mismatches += 1;
                if previous != Some(signature) {
                    score.patch_runs += 1;
                }
            }
            previous = signature;
        }
        Ok(score)
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
