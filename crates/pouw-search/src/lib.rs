#![forbid(unsafe_code)]

mod baseline;
mod checkpoint;
mod genetics;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

use pouw_core::{
    candidate_encoding_hash, decode_candidate, import_incumbent, semantic_root, verify_result,
    CandidateProgram, Error, ErrorKind, Hash32, LimitsV1, Result, ResultV1, SearchMetadataV1,
    Semantics, TaskV1, VmStats,
};
use serde::{Deserialize, Serialize};

pub use baseline::{baseline_candidates, best_baseline};
pub use checkpoint::CheckpointV1;

pub const SEARCH_ENGINE_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    pub seed: u64,
    pub threads: u16,
    pub islands: u16,
    pub population: u32,
    pub generations: u32,
    pub epoch_generations: u16,
    pub elite_count: u16,
    pub tournament_size: u8,
    pub max_attempts: Option<u64>,
    pub time_limit_ms: Option<u64>,
    pub memory_limit_bytes: u64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        let threads = default_thread_count();
        Self {
            seed: 1,
            threads,
            islands: threads.min(8),
            population: 64,
            generations: 200,
            epoch_generations: 4,
            elite_count: 4,
            tournament_size: 3,
            max_attempts: None,
            time_limit_ms: None,
            memory_limit_bytes: 512 * 1024 * 1024,
        }
    }
}

impl SearchConfig {
    pub fn validate(&self, limits: &LimitsV1) -> Result<()> {
        if self.threads == 0
            || self.islands == 0
            || self.population < 4
            || self.population > 16_384
            || self.generations == 0
            || self.epoch_generations == 0
            || self.elite_count == 0
            || u32::from(self.elite_count) >= self.population
            || self.tournament_size < 2
            || u32::from(self.tournament_size) > self.population
            || self.max_attempts == Some(0)
            || self.time_limit_ms == Some(0)
            || self.memory_limit_bytes < 1024 * 1024
            || self.memory_limit_bytes > limits.max_memory_bytes.saturating_mul(16)
        {
            return Err(Error::limit(
                "invalid-search-config",
                "Search configuration is outside the v1 bounded envelope.",
            ));
        }
        let estimate = u64::from(self.population)
            .checked_mul(u64::from(self.islands))
            .and_then(|value| value.checked_mul(u64::from(limits.max_input_bytes.min(65_536))))
            .ok_or_else(|| Error::overflow("Search memory estimate overflow."))?;
        if estimate > self.memory_limit_bytes {
            return Err(Error::limit(
                "search-memory-limit",
                "Population and island configuration exceeds the search memory budget.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchCandidate {
    pub program: CandidateProgram,
    pub encoding: Vec<u8>,
    pub encoding_hash: Hash32,
    pub semantic_root: Hash32,
    pub mismatch_count: u64,
    pub exact: bool,
    pub stats: VmStats,
}

impl SearchCandidate {
    pub fn stored_bytes(&self) -> u32 {
        self.stats.total_bytes
    }

    pub fn fitness_cmp(&self, other: &Self) -> Ordering {
        match (self.exact, other.exact) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => self
                .mismatch_count
                .cmp(&other.mismatch_count)
                .then_with(|| self.stats.total_bytes.cmp(&other.stats.total_bytes))
                .then_with(|| self.stats.decode_units.cmp(&other.stats.decode_units))
                .then_with(|| self.encoding.cmp(&other.encoding)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgress {
    pub generation: u32,
    pub attempts: u64,
    pub elapsed_ms: u64,
    pub attempts_per_second_milli: u64,
    pub best_bytes: u32,
    pub program_bytes: u32,
    pub residual_bytes: u32,
    pub decode_units: u64,
    pub exact: bool,
    pub semantic_root: Hash32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub best: SearchCandidate,
    pub result: ResultV1,
    pub improved: bool,
    pub attempts: u64,
    pub elapsed_ms: u64,
    pub generations: u32,
    pub checkpoint: CheckpointV1,
}

#[derive(Clone, Default)]
pub struct SearchControl {
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

impl SearchControl {
    pub fn stop(&self) {
        self.stop.store(true, AtomicOrdering::Release);
    }

    pub fn pause(&self) {
        self.pause.store(true, AtomicOrdering::Release);
    }

    pub fn resume(&self) {
        self.pause.store(false, AtomicOrdering::Release);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(AtomicOrdering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(AtomicOrdering::Acquire)
    }
}

#[derive(Clone)]
struct IslandState {
    index: u16,
    generation: u32,
    attempts: u64,
    population: Vec<SearchCandidate>,
}

/// Search timing is non-consensus metadata. `std::time::Instant` deliberately
/// traps on `wasm32-unknown-unknown`, so browser workers use bounded generation
/// slices and let JavaScript enforce the wall-clock budget between slices.
struct SearchClock {
    #[cfg(not(target_arch = "wasm32"))]
    started: Instant,
    #[cfg(not(target_arch = "wasm32"))]
    deadline: Option<Instant>,
}

impl SearchClock {
    fn start(time_limit_ms: Option<u64>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let started = Instant::now();
            Self {
                started,
                deadline: time_limit_ms.map(|value| started + Duration::from_millis(value)),
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = time_limit_ms;
            Self {}
        }
    }

    fn elapsed_ms(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
        }

        #[cfg(target_arch = "wasm32")]
        {
            0
        }
    }

    fn deadline_reached(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.deadline.is_some_and(|value| Instant::now() >= value)
        }

        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }
}

pub fn mine<F>(
    task: &TaskV1,
    config: &SearchConfig,
    control: &SearchControl,
    mut progress: F,
) -> Result<SearchOutcome>
where
    F: FnMut(&SearchProgress),
{
    mine_from_checkpoint(task, config, control, None, &mut progress)
}

pub fn resume<F>(
    task: &TaskV1,
    checkpoint: &CheckpointV1,
    control: &SearchControl,
    mut progress: F,
) -> Result<SearchOutcome>
where
    F: FnMut(&SearchProgress),
{
    checkpoint.validate_for_task(task)?;
    mine_from_checkpoint(
        task,
        &checkpoint.config,
        control,
        Some(checkpoint),
        &mut progress,
    )
}

fn mine_from_checkpoint(
    task: &TaskV1,
    config: &SearchConfig,
    control: &SearchControl,
    checkpoint: Option<&CheckpointV1>,
    progress: &mut dyn FnMut(&SearchProgress),
) -> Result<SearchOutcome> {
    task.validate()?;
    config.validate(&task.limits)?;
    let target = import_incumbent(
        task.profile,
        task.incumbent_format,
        &task.incumbent_encoding,
        &task.limits,
    )?;
    let mut baselines = baseline_candidates(&target, &task.limits)?;
    if baselines.is_empty() {
        return Err(Error::new(
            ErrorKind::Internal,
            "search-no-baseline",
            "Search could not construct an exact baseline.",
        ));
    }
    baselines.sort_by(SearchCandidate::fitness_cmp);
    let resume_best = checkpoint
        .map(|value| value.best_candidate(task, &target))
        .transpose()?;
    if let Some(candidate) = resume_best {
        baselines.push(candidate);
        baselines.sort_by(SearchCandidate::fitness_cmp);
    }
    let start_generation = checkpoint.map_or(0, |value| value.generation);
    let mut attempts = checkpoint.map_or(0, |value| value.attempts);
    let island_count = usize::from(config.islands.max(1));
    let mut islands = (0..island_count)
        .map(|index| IslandState {
            index: index as u16,
            generation: start_generation,
            attempts: 0,
            population: seed_population(&baselines, config.population as usize, index),
        })
        .collect::<Vec<_>>();
    let clock = SearchClock::start(config.time_limit_ms);
    let mut completed_generation = start_generation;
    let end_generation = start_generation.saturating_add(config.generations);
    let mut global_best = baselines[0].clone();

    while completed_generation < end_generation {
        if control.is_stopped() || budget_exhausted(attempts, config, &clock) {
            break;
        }
        while control.is_paused() && !control.is_stopped() {
            pause_briefly();
            if clock.deadline_reached() {
                break;
            }
        }
        if control.is_stopped() || budget_exhausted(attempts, config, &clock) {
            break;
        }
        let epoch = u32::from(config.epoch_generations).min(end_generation - completed_generation);
        for island in &mut islands {
            inject_migrant(&mut island.population, &global_best);
        }
        run_island_epoch(&mut islands, &target, &task.limits, config, epoch)?;
        let epoch_attempts = islands
            .iter_mut()
            .map(|island| {
                let value = island.attempts;
                island.attempts = 0;
                value
            })
            .sum::<u64>();
        attempts = attempts.saturating_add(epoch_attempts);
        completed_generation = completed_generation.saturating_add(epoch);
        for island in &islands {
            if let Some(candidate) = island.population.first() {
                if candidate.fitness_cmp(&global_best).is_lt() {
                    global_best = candidate.clone();
                }
            }
        }
        let elapsed_ms = clock.elapsed_ms();
        progress(&progress_snapshot(
            completed_generation,
            attempts,
            elapsed_ms,
            &global_best,
        ));
    }

    let elapsed_ms = clock.elapsed_ms();
    let result = ResultV1::create(
        task,
        global_best.encoding.clone(),
        None,
        Some(SearchMetadataV1 {
            algorithm: "typed-island-gp-v1".into(),
            attempts,
            elapsed_ms,
            seed: config.seed,
            threads: config.threads,
        }),
    )?;
    let report = verify_result(task, &result)?;
    if !report.exact {
        return Err(Error::new(
            ErrorKind::Internal,
            "search-final-verification",
            "Search returned a candidate that failed independent exact verification.",
        ));
    }
    let checkpoint = CheckpointV1::new(
        task,
        config.clone(),
        completed_generation,
        attempts,
        &global_best,
    )?;
    Ok(SearchOutcome {
        best: global_best,
        result,
        improved: report.improved,
        attempts,
        elapsed_ms,
        generations: completed_generation,
        checkpoint,
    })
}

#[cfg(feature = "parallel")]
fn run_island_epoch(
    islands: &mut [IslandState],
    target: &Semantics,
    limits: &LimitsV1,
    config: &SearchConfig,
    epoch: u32,
) -> Result<()> {
    use rayon::prelude::*;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(usize::from(config.threads))
        .build()
        .map_err(|error| Error::new(ErrorKind::Internal, "rayon-pool", error.to_string()))?;
    pool.install(|| {
        islands
            .par_iter_mut()
            .try_for_each(|island| genetics::evolve_epoch(island, target, limits, config, epoch))
    })
}

#[cfg(not(feature = "parallel"))]
fn run_island_epoch(
    islands: &mut [IslandState],
    target: &Semantics,
    limits: &LimitsV1,
    config: &SearchConfig,
    epoch: u32,
) -> Result<()> {
    for island in islands {
        genetics::evolve_epoch(island, target, limits, config, epoch)?;
    }
    Ok(())
}

fn seed_population(
    baselines: &[SearchCandidate],
    population_size: usize,
    island: usize,
) -> Vec<SearchCandidate> {
    let mut output = Vec::with_capacity(population_size);
    for index in 0..population_size {
        output.push(baselines[(index + island) % baselines.len()].clone());
    }
    output.sort_by(SearchCandidate::fitness_cmp);
    output
}

fn inject_migrant(population: &mut Vec<SearchCandidate>, migrant: &SearchCandidate) {
    if population
        .iter()
        .any(|value| value.encoding == migrant.encoding)
    {
        return;
    }
    if let Some(last) = population.last_mut() {
        *last = migrant.clone();
    } else {
        population.push(migrant.clone());
    }
    population.sort_by(SearchCandidate::fitness_cmp);
}

fn budget_exhausted(attempts: u64, config: &SearchConfig, clock: &SearchClock) -> bool {
    config
        .max_attempts
        .is_some_and(|maximum| attempts >= maximum)
        || clock.deadline_reached()
}

fn pause_briefly() {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(Duration::from_millis(20));

    #[cfg(target_arch = "wasm32")]
    core::hint::spin_loop();
}

fn progress_snapshot(
    generation: u32,
    attempts: u64,
    elapsed_ms: u64,
    best: &SearchCandidate,
) -> SearchProgress {
    let attempts_per_second_milli = if elapsed_ms == 0 {
        0
    } else {
        attempts.saturating_mul(1_000_000) / elapsed_ms
    };
    SearchProgress {
        generation,
        attempts,
        elapsed_ms,
        attempts_per_second_milli,
        best_bytes: best.stats.total_bytes,
        program_bytes: best.stats.program_bytes,
        residual_bytes: best.stats.residual_bytes,
        decode_units: best.stats.decode_units,
        exact: best.exact,
        semantic_root: best.semantic_root,
    }
}

pub(crate) fn evaluate_program(
    program: CandidateProgram,
    target: &Semantics,
    limits: &LimitsV1,
) -> Result<SearchCandidate> {
    let program = baseline::exactify(program, target, limits)?;
    let encoding = pouw_core::encode_candidate(&program, limits)?;
    let decoded = decode_candidate(&encoding, target.profile(), limits)?;
    let mismatch_count = target.mismatch_count(&decoded.semantics);
    let candidate_root = semantic_root(&decoded.semantics);
    let expected_root = semantic_root(target);
    let exact = mismatch_count == 0 && candidate_root == expected_root;
    if !exact {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "search-candidate-mismatch",
            "Exact residual generation did not reproduce the target semantics.",
        ));
    }
    Ok(SearchCandidate {
        encoding_hash: candidate_encoding_hash(target.profile(), &encoding),
        encoding,
        semantic_root: candidate_root,
        mismatch_count,
        exact,
        stats: decoded.stats,
        program,
    })
}

fn default_thread_count() -> u16 {
    std::thread::available_parallelism()
        .map(|value| value.get().saturating_sub(1).max(1))
        .unwrap_or(1)
        .min(usize::from(u16::MAX)) as u16
}

extern crate alloc;

#[cfg(test)]
mod tests {
    use super::*;
    use pouw_core::{
        import_asset, Coord, ForgeComponent, ForgeEquipment, ForgeGeometry, ForgedSemantics,
        Profile, TerrainSemantics,
    };

    fn terrain_task() -> TaskV1 {
        let capacity = 64_u16;
        let count = 16_u16;
        let mut account = vec![0_u8; 16 + usize::from(capacity) * 3];
        account[0..4].copy_from_slice(b"NCBK");
        account[4] = 1;
        account[6..8].copy_from_slice(&count.to_le_bytes());
        account[8..10].copy_from_slice(&capacity.to_le_bytes());
        for index in 0..usize::from(count) {
            let packed = index as u32;
            let offset = 16 + index * 3;
            account[offset] = packed as u8;
            account[offset + 1] = (packed >> 8) as u8;
            account[offset + 2] = (packed >> 16) as u8;
        }
        let limits = LimitsV1::default();
        let imported = import_asset(Profile::TerrainDelta, &account, &limits).unwrap();
        TaskV1::create(imported, "search:test", limits, None).unwrap()
    }

    #[test]
    fn fixed_seed_single_thread_search_is_reproducible() {
        let task = terrain_task();
        let config = SearchConfig {
            seed: 123,
            threads: 1,
            islands: 1,
            population: 8,
            generations: 4,
            epoch_generations: 1,
            elite_count: 2,
            tournament_size: 2,
            max_attempts: None,
            time_limit_ms: None,
            memory_limit_bytes: 32 * 1024 * 1024,
        };
        let first = mine(&task, &config, &SearchControl::default(), |_| {}).unwrap();
        let second = mine(&task, &config, &SearchControl::default(), |_| {}).unwrap();
        assert_eq!(first.best.encoding, second.best.encoding);
        assert_eq!(first.attempts, second.attempts);
        assert!(first.best.exact);
        assert!(first.improved);
    }

    #[test]
    fn all_profile_baselines_are_exact() {
        let limits = LimitsV1::default();
        let terrain = Semantics::TerrainDelta(TerrainSemantics {
            min_y: -12,
            deleted: (0..8).map(|x| Coord { x, y: 2, z: 1 }).collect(),
        });
        assert!(best_baseline(&terrain, &limits).unwrap().exact);

        let forged = Semantics::ForgedItem(ForgedSemantics {
            equipment: ForgeEquipment {
                mass_5g: 10,
                encoded_volume: 20,
                attributes_6: [2; 12],
            },
            geometry: ForgeGeometry::Components {
                components: vec![ForgeComponent {
                    resource: 0,
                    color_444: 0x9aa,
                    dimensions_q: [64, 64, 64],
                    offset_q: [0, 0, 0],
                    grip: None,
                    solid: (0..100).collect(),
                    paint: vec![],
                }],
            },
        });
        assert!(best_baseline(&forged, &limits).unwrap().exact);
    }
}
