#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use pouw_core::{
    candidate_encoding_hash, decode_candidate, decode_ncm4, detect_format, deterministic_ncm4_seed,
    encoding_hash, hash_hex, import_asset, import_incumbent, semantic_root, verify_result,
    DetectedFormat, Error, ErrorKind, LimitsV1, Profile, Result as CoreResult, ResultV1,
    SearchMetadataV1, TaskV1, VerificationReportV1, COST_MODEL_VERSION, NCM4_VERSION,
    PROTOCOL_VERSION, VM_VERSION,
};
use pouw_search::{
    best_baseline, mine, resume, CheckpointV1, Ncm4SearchCheckpoint, Ncm4SearchProgress,
    Ncm4SearchSession, SearchConfig, SearchControl, SearchProgress,
};
use serde_json::{json, Value};
use tempfile::NamedTempFile;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit ",
    env!("NICECHUNK_GIT_COMMIT"),
    ", protocol 1, VM 1, cost model 1)"
);

#[derive(Parser, Debug)]
#[command(
    name = "nicechunk-miner",
    version = LONG_VERSION,
    about = "NiceChunk Proof of Useful Work Miner with NCM4 Alpha",
    long_about = "Deterministically compress, search, decode, and independently verify exact NiceChunk voxel assets with unchanged NCM3 compatibility."
)]
struct Cli {
    #[arg(long, global = true, help = "Emit stable JSON reports on stdout")]
    json: bool,

    #[arg(
        long,
        global = true,
        help = "Emit newline-delimited JSON progress on stderr"
    )]
    json_progress: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Inspect(InspectArgs),
    Ncm4(Ncm4Args),
    Task(TaskArgs),
    Baseline(BaselineArgs),
    Mine(MineArgs),
    Resume(ResumeArgs),
    Verify(VerifyArgs),
    Decode(DecodeArgs),
    Benchmark(BenchmarkArgs),
    SelfTest,
}

#[derive(Args, Debug)]
struct InspectArgs {
    input: PathBuf,
    #[arg(long)]
    profile: Option<Profile>,
}

#[derive(Args, Debug)]
struct Ncm4Args {
    #[command(subcommand)]
    command: Ncm4Command,
}

#[derive(Subcommand, Debug)]
enum Ncm4Command {
    Analyze(Ncm4AnalyzeArgs),
    Encode(Ncm4EncodeArgs),
    Decode(Ncm4DecodeArgs),
    Verify(Ncm4VerifyArgs),
}

#[derive(Args, Debug)]
struct Ncm4AnalyzeArgs {
    input: PathBuf,
    #[arg(long)]
    profile: Option<Profile>,
}

#[derive(Args, Debug)]
struct Ncm4EncodeArgs {
    input: PathBuf,
    #[arg(long)]
    profile: Option<Profile>,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args, Debug)]
struct Ncm4DecodeArgs {
    input: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args, Debug)]
struct Ncm4VerifyArgs {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    candidate: PathBuf,
    #[arg(long)]
    profile: Option<Profile>,
}

#[derive(Args, Debug)]
struct TaskArgs {
    #[command(subcommand)]
    command: TaskCommand,
}

#[derive(Subcommand, Debug)]
enum TaskCommand {
    Create(TaskCreateArgs),
}

#[derive(Args, Debug)]
struct TaskCreateArgs {
    #[arg(long)]
    profile: Profile,
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value = "local:asset")]
    asset_id: String,
}

#[derive(Args, Debug)]
struct BaselineArgs {
    #[arg(long)]
    task: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args, Debug)]
struct MineArgs {
    #[arg(value_name = "INPUT", conflicts_with_all = ["task", "resume"])]
    input: Option<PathBuf>,
    #[arg(long, conflicts_with = "resume")]
    task: Option<PathBuf>,
    #[arg(long, conflicts_with = "task")]
    resume: Option<PathBuf>,
    #[arg(long, default_value = "auto")]
    threads: String,
    #[arg(
        long,
        help = "Persistent island count (defaults to resolved thread count)"
    )]
    islands: Option<u16>,
    #[arg(long, default_value_t = 64)]
    population: u32,
    #[arg(long, default_value_t = 200)]
    generations: u32,
    #[arg(long)]
    time_limit: Option<String>,
    #[arg(long)]
    max_attempts: Option<u64>,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = 0)]
    shard_index: u32,
    #[arg(long, default_value_t = 1)]
    shard_count: u32,
    #[arg(long)]
    checkpoint: Option<PathBuf>,
    #[arg(long, default_value = "result.ncpow")]
    out: PathBuf,
}

#[derive(Args, Debug)]
struct ResumeArgs {
    checkpoint: PathBuf,
    #[arg(long, default_value = "result.ncpow")]
    out: PathBuf,
    #[arg(long)]
    checkpoint_out: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct VerifyArgs {
    #[arg(long)]
    task: PathBuf,
    #[arg(long)]
    result: PathBuf,
}

#[derive(Args, Debug)]
struct DecodeArgs {
    #[arg(long)]
    result: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args, Debug)]
struct BenchmarkArgs {
    #[arg(long, default_value = "test-vectors")]
    corpus: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            ExitCode::from(exit_code(error.kind))
        }
    }
}

fn run(cli: Cli) -> CoreResult<()> {
    match cli.command {
        Command::Inspect(args) => inspect(args, cli.json),
        Command::Ncm4(args) => match args.command {
            Ncm4Command::Analyze(args) => ncm4_analyze(args, cli.json),
            Ncm4Command::Encode(args) => ncm4_encode(args, cli.json),
            Ncm4Command::Decode(args) => ncm4_decode(args, cli.json),
            Ncm4Command::Verify(args) => ncm4_verify(args, cli.json),
        },
        Command::Task(args) => match args.command {
            TaskCommand::Create(args) => create_task(args, cli.json),
        },
        Command::Baseline(args) => baseline(args, cli.json),
        Command::Mine(args) => mine_command(args, cli.json, cli.json_progress),
        Command::Resume(args) => mine_command(
            MineArgs {
                input: None,
                task: None,
                resume: Some(args.checkpoint),
                threads: "auto".into(),
                islands: None,
                population: 64,
                generations: 200,
                time_limit: None,
                max_attempts: None,
                seed: 1,
                shard_index: 0,
                shard_count: 1,
                checkpoint: args.checkpoint_out,
                out: args.out,
            },
            cli.json,
            cli.json_progress,
        ),
        Command::Verify(args) => verify(args, cli.json),
        Command::Decode(args) => decode(args, cli.json),
        Command::Benchmark(args) => benchmark(args, cli.json),
        Command::SelfTest => self_test(cli.json),
    }
}

fn inspect(args: InspectArgs, json_output: bool) -> CoreResult<()> {
    let input = read_path(&args.input)?;
    let limits = LimitsV1::default();
    let profile = resolve_profile(&args.input, &input, args.profile, &limits)?;
    let imported = import_asset(profile, &input, &limits)?;
    let root = semantic_root(&imported.semantics);
    let hash = encoding_hash(
        imported.profile,
        imported.format,
        &imported.incumbent_encoding,
    );
    let report = json!({
        "profile": imported.profile.as_str(),
        "format": imported.format.as_str(),
        "inputBytes": imported.incumbent_encoding.len(),
        "semanticRoot": hash_hex(&root),
        "encodingHash": hash_hex(&hash),
        "voxelCount": imported.semantics.voxel_count(),
        "semantics": imported.semantics,
    });
    print_report(&report, json_output)
}

fn ncm4_analyze(args: Ncm4AnalyzeArgs, json_output: bool) -> CoreResult<()> {
    let input = read_path(&args.input)?;
    let limits = LimitsV1::default();
    let profile = resolve_profile(&args.input, &input, args.profile, &limits)?;
    let imported = import_asset(profile, &input, &limits)?;
    if imported.format == pouw_core::IncumbentFormat::Ncm4PouwV1 {
        let decoded = decode_ncm4(&imported.incumbent_encoding, &limits)?;
        return print_report(
            &json!({
                "inputFormat": imported.format.as_str(),
                "profile": profile.as_str(),
                "semanticRoot": hash_hex(&decoded.semantic_root),
                "encodingHash": hash_hex(&decoded.encoding_hash),
                "exact": true,
                "witnessExists": false,
                "selectedFormat": imported.format.as_str(),
                "ncm4": decoded.stats,
            }),
            json_output,
        );
    }
    let seed = deterministic_ncm4_seed(&imported, &limits)?;
    print_report(&ncm4_seed_json(&seed), json_output)
}

fn ncm4_encode(args: Ncm4EncodeArgs, json_output: bool) -> CoreResult<()> {
    let input = read_path(&args.input)?;
    let limits = LimitsV1::default();
    let profile = resolve_profile(&args.input, &input, args.profile, &limits)?;
    let imported = import_asset(profile, &input, &limits)?;
    if imported.format == pouw_core::IncumbentFormat::Ncm4PouwV1 {
        return Err(Error::invalid(
            "ncm4-already-encoded",
            "Input is already an NCM4 PoUW encoding.",
        ));
    }
    let seed = deterministic_ncm4_seed(&imported, &limits)?;
    write_atomic(&args.out, &seed.encoding)?;
    let report = merge_json(ncm4_seed_json(&seed), json!({ "output": args.out }));
    print_report(&report, json_output)
}

fn ncm4_decode(args: Ncm4DecodeArgs, json_output: bool) -> CoreResult<()> {
    let limits = LimitsV1::default();
    let decoded = decode_ncm4(&read_path(&args.input)?, &limits)?;
    let report = json!({
        "format": "ncm4-pouw-v1",
        "profile": decoded.profile.as_str(),
        "semanticRoot": hash_hex(&decoded.semantic_root),
        "encodingHash": hash_hex(&decoded.encoding_hash),
        "stats": decoded.stats,
        "semantics": decoded.semantics,
    });
    write_atomic(
        &args.out,
        &serde_json::to_vec_pretty(&report).map_err(json_error)?,
    )?;
    print_report(
        &json!({ "output": args.out, "report": report }),
        json_output,
    )
}

fn ncm4_verify(args: Ncm4VerifyArgs, json_output: bool) -> CoreResult<()> {
    let limits = LimitsV1::default();
    let source_bytes = read_path(&args.source)?;
    let profile = resolve_profile(&args.source, &source_bytes, args.profile, &limits)?;
    let source = import_asset(profile, &source_bytes, &limits)?;
    let candidate = decode_ncm4(&read_path(&args.candidate)?, &limits)?;
    let target_root = semantic_root(&source.semantics);
    let mismatch_count = source.semantics.mismatch_count(&candidate.semantics);
    let exact = candidate.profile == profile
        && candidate.semantic_root == target_root
        && mismatch_count == 0;
    let source_size = source.incumbent_encoding.len() as i64;
    let candidate_size = i64::from(candidate.stats.total_bytes);
    let improved = exact && candidate_size < source_size;
    let report = json!({
        "accepted": improved,
        "exact": exact,
        "improved": improved,
        "mismatchCount": mismatch_count,
        "sourceFormat": source.format.as_str(),
        "candidateFormat": "ncm4-pouw-v1",
        "profile": profile.as_str(),
        "targetSemanticRoot": hash_hex(&target_root),
        "candidateSemanticRoot": hash_hex(&candidate.semantic_root),
        "candidateEncodingHash": hash_hex(&candidate.encoding_hash),
        "sourceBytes": source_size,
        "candidateBytes": candidate_size,
        "savedBytes": source_size - candidate_size,
        "stats": candidate.stats,
        "selectedFormat": if improved { "ncm4-pouw-v1" } else { source.format.as_str() },
    });
    print_report(&report, json_output)?;
    if !exact {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "ncm4-semantic-mismatch",
            "NCM4 candidate is not exactly equivalent to the source.",
        ));
    }
    if !improved {
        return Err(Error::new(
            ErrorKind::NotSmaller,
            "ncm4-not-smaller",
            "NCM4 candidate is exact but does not beat the source representation.",
        ));
    }
    Ok(())
}

fn ncm4_seed_json(seed: &pouw_core::Ncm4Seed) -> Value {
    json!({
        "inputFormat": seed.audit.source_format,
        "profile": seed.audit.profile.as_str(),
        "semanticRoot": hash_hex(&seed.audit.semantic_root),
        "candidateSemanticRoot": hash_hex(&seed.audit.candidate_semantic_root),
        "encodingHash": hash_hex(&seed.decoded.encoding_hash),
        "sourceBytes": seed.audit.source_bytes,
        "fixedHeaderBytes": seed.audit.fixed_header_bytes,
        "profileHeaderBytes": seed.audit.profile_header_bytes,
        "bodyBytes": seed.audit.body_bytes,
        "residualBytes": seed.audit.residual_bytes,
        "patches": seed.decoded.stats.patches,
        "ncm4TotalBytes": seed.audit.ncm4_total_bytes,
        "theoreticalFixedLowerBound": seed.audit.theoretical_fixed_lower_bound,
        "deterministicSeedBytes": seed.audit.deterministic_seed_bytes,
        "savedBytes": seed.audit.saved_bytes,
        "savedBps": seed.audit.saved_basis_points,
        "decodeUnits": seed.decoded.stats.decode_units,
        "exact": seed.audit.exact,
        "witnessExists": seed.audit.witness_exists,
        "recommendDeepSearch": seed.audit.recommend_deep_search,
        "selectedFormat": seed.audit.selected_format,
        "ncm4Version": NCM4_VERSION,
    })
}

fn create_task(args: TaskCreateArgs, json_output: bool) -> CoreResult<()> {
    let input = read_path(&args.input)?;
    let limits = LimitsV1::default();
    let imported = import_asset(args.profile, &input, &limits)?;
    let task = TaskV1::create(imported, args.asset_id, limits, None)?;
    let bytes = task.to_bytes()?;
    write_atomic(&args.out, &bytes)?;
    let report = json!({
        "taskId": hash_hex(&task.id()?),
        "profile": task.profile.as_str(),
        "assetId": task.asset_id,
        "semanticRoot": hash_hex(&task.semantic_root),
        "incumbentEncodingHash": hash_hex(&task.incumbent_encoding_hash),
        "incumbentBytes": task.incumbent_encoding.len(),
        "output": args.out,
    });
    print_report(&report, json_output)
}

fn baseline(args: BaselineArgs, json_output: bool) -> CoreResult<()> {
    let task = read_task(&args.task)?;
    let target = import_incumbent(
        task.profile,
        task.incumbent_format,
        &task.incumbent_encoding,
        &task.limits,
    )?;
    let started = Instant::now();
    let candidate = best_baseline(&target, &task.limits)?;
    let result = ResultV1::create(
        &task,
        candidate.encoding,
        None,
        Some(SearchMetadataV1 {
            algorithm: "deterministic-baseline-v1".into(),
            attempts: 1,
            elapsed_ms: elapsed_ms(started),
            seed: 0,
            threads: 1,
        }),
    )?;
    let report = verify_result(&task, &result)?;
    write_atomic(&args.out, &result.to_bytes()?)?;
    print_verification(&report, Some(&args.out), json_output)
}

fn mine_command(args: MineArgs, json_output: bool, json_progress: bool) -> CoreResult<()> {
    let ncm4_resume_bytes = args
        .resume
        .as_ref()
        .map(|path| read_path(path))
        .transpose()?
        .filter(|bytes| bytes.starts_with(b"NC4S1\n"));
    if args.input.is_some() || ncm4_resume_bytes.is_some() {
        return mine_ncm4_command(args, ncm4_resume_bytes, json_output, json_progress);
    }
    let (task, checkpoint, config) = if let Some(path) = &args.resume {
        let checkpoint = CheckpointV1::from_bytes(&read_path(path)?)?;
        let task = TaskV1::from_bytes(&checkpoint.task_bytes)?;
        checkpoint.validate_for_task(&task)?;
        (task, Some(checkpoint.clone()), checkpoint.config)
    } else {
        let task_path = args
            .task
            .as_ref()
            .ok_or_else(|| Error::invalid("mine-task", "Mining requires --task or --resume."))?;
        let task = read_task(task_path)?;
        let threads = parse_threads(&args.threads)?;
        let config = SearchConfig {
            seed: args.seed,
            threads,
            islands: args.islands.unwrap_or(threads),
            population: args.population,
            generations: args.generations,
            elite_count: (args.population / 16).clamp(1, u32::from(u16::MAX)) as u16,
            tournament_size: args.population.clamp(2, 3) as u8,
            max_attempts: args.max_attempts,
            time_limit_ms: args.time_limit.as_deref().map(parse_duration).transpose()?,
            shard_index: args.shard_index,
            shard_count: args.shard_count,
            ..SearchConfig::default()
        };
        (task, None, config)
    };

    let control = SearchControl::default();
    let signal_control = control.clone();
    ctrlc::set_handler(move || signal_control.stop())
        .map_err(|error| Error::new(ErrorKind::Internal, "ctrl-c-handler", error.to_string()))?;
    let mut last_progress_ms: Option<u64> = None;
    let progress_callback = |progress: &SearchProgress| {
        if last_progress_ms.is_some_and(|last| progress.elapsed_ms < last.saturating_add(250)) {
            return;
        }
        last_progress_ms = Some(progress.elapsed_ms);
        if json_progress {
            let value = json!({
                "type": "progress",
                "generation": progress.generation,
                "attempts": progress.attempts,
                "attemptsPerSecond": progress.attempts_per_second_milli as f64 / 1000.0,
                "elapsedMs": progress.elapsed_ms,
                "programBytes": progress.program_bytes,
                "residualBytes": progress.residual_bytes,
                "totalBytes": progress.best_bytes,
                "decodeUnits": progress.decode_units,
                "semanticRoot": hash_hex(&progress.semantic_root),
                "exact": progress.exact,
            });
            eprintln!(
                "{}",
                serde_json::to_string(&value).unwrap_or_else(|_| "{}".into())
            );
        } else {
            eprintln!(
                "generation={} attempts={} rate={:.3}/s elapsed={:.3}s bytes={} (program={}, residual={}) decodeUnits={} exact={}",
                progress.generation,
                progress.attempts,
                progress.attempts_per_second_milli as f64 / 1000.0,
                progress.elapsed_ms as f64 / 1000.0,
                progress.best_bytes,
                progress.program_bytes,
                progress.residual_bytes,
                progress.decode_units,
                progress.exact,
            );
        }
    };
    let outcome = match &checkpoint {
        Some(value) => resume(&task, value, &control, progress_callback)?,
        None => mine(&task, &config, &control, progress_callback)?,
    };
    write_atomic(&args.out, &outcome.result.to_bytes()?)?;
    if let Some(path) = &args.checkpoint {
        write_atomic(path, &outcome.checkpoint.to_bytes()?)?;
    } else if control.is_stopped() {
        let fallback = args.out.with_extension("chk");
        write_atomic(&fallback, &outcome.checkpoint.to_bytes()?)?;
        eprintln!("checkpoint={}", fallback.display());
    }
    let report = verify_result(&task, &outcome.result)?;
    let value = verification_json(&report, Some(&args.out));
    let value = merge_json(
        value,
        json!({
            "attempts": outcome.attempts,
            "elapsedMs": outcome.elapsed_ms,
            "generations": outcome.generations,
            "stopped": control.is_stopped(),
            "checkpoint": args.checkpoint,
        }),
    );
    print_report(&value, json_output)
}

fn mine_ncm4_command(
    args: MineArgs,
    resume_bytes: Option<Vec<u8>>,
    json_output: bool,
    json_progress: bool,
) -> CoreResult<()> {
    let limits = LimitsV1::default();
    let mut session = if let Some(bytes) = resume_bytes {
        let checkpoint = Ncm4SearchCheckpoint::from_bytes(&bytes)?;
        Ncm4SearchSession::from_checkpoint(&checkpoint)?
    } else {
        let path = args.input.as_ref().ok_or_else(|| {
            Error::invalid(
                "ncm4-mine-input",
                "NCM4 mining requires an input asset or NCM4 search checkpoint.",
            )
        })?;
        let input = read_path(path)?;
        let profile = resolve_profile(path, &input, None, &limits)?;
        if profile != Profile::Building {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "ncm4-search-profile",
                "NCM4 alpha deep search supports building inputs; other profiles still use the v1 task miner.",
            ));
        }
        let imported = import_asset(profile, &input, &limits)?;
        if !matches!(
            imported.format,
            pouw_core::IncumbentFormat::Ncm3V1 | pouw_core::IncumbentFormat::Ncm4PouwV1
        ) {
            return Err(Error::new(
                ErrorKind::UnsupportedVersion,
                "ncm4-search-format",
                "NCM4 alpha deep search supports NCM3 and NCM4 PoUW building inputs.",
            ));
        }
        let threads = parse_threads(&args.threads)?;
        let config = SearchConfig {
            seed: args.seed,
            threads,
            islands: args.islands.unwrap_or(threads),
            population: args.population,
            generations: args.generations,
            epoch_generations: 1,
            elite_count: (args.population / 16).clamp(1, u32::from(u16::MAX)) as u16,
            tournament_size: args.population.clamp(2, 3) as u8,
            max_attempts: args.max_attempts,
            time_limit_ms: args.time_limit.as_deref().map(parse_duration).transpose()?,
            memory_limit_bytes: 512 * 1024 * 1024,
            shard_index: args.shard_index,
            shard_count: args.shard_count,
        };
        Ncm4SearchSession::new(imported, config)?
    };

    let control = SearchControl::default();
    let signal_control = control.clone();
    ctrlc::set_handler(move || signal_control.stop())
        .map_err(|error| Error::new(ErrorKind::Internal, "ctrl-c-handler", error.to_string()))?;
    let started = Instant::now();
    let start_generation = session.generation();
    let target_generation = start_generation.saturating_add(session.config().generations);
    while session.generation() < target_generation && !control.is_stopped() {
        if session
            .config()
            .max_attempts
            .is_some_and(|maximum| session.attempts() >= maximum)
            || session
                .config()
                .time_limit_ms
                .is_some_and(|maximum| elapsed_ms(started) >= maximum)
        {
            break;
        }
        let remaining = target_generation - session.generation();
        let epoch = u32::from(session.config().epoch_generations).min(remaining);
        session.step(epoch, |progress| {
            print_ncm4_progress(progress, json_progress, elapsed_ms(started));
        })?;
    }
    let checkpoint = session.checkpoint()?;
    let best = session.best().clone();
    let independently_decoded = decode_ncm4(&best.encoding, &limits)?;
    if independently_decoded.semantic_root != best.semantic_root {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "ncm4-final-verification",
            "NCM4 CLI final candidate failed independent verification.",
        ));
    }
    write_atomic(&args.out, &best.encoding)?;
    let checkpoint_path = if let Some(path) = &args.checkpoint {
        write_atomic(path, &checkpoint.to_bytes()?)?;
        Some(path.clone())
    } else if control.is_stopped() {
        let path = args.out.with_extension("nc4s.chk");
        write_atomic(&path, &checkpoint.to_bytes()?)?;
        Some(path)
    } else {
        None
    };
    let source_bytes = session.source_bytes();
    let saved_bytes = i64::from(source_bytes) - i64::from(best.stats.total_bytes);
    let improved = best.stats.total_bytes < source_bytes;
    let report = json!({
        "format": "ncm4-pouw-v1",
        "exact": best.exact,
        "improved": improved,
        "witnessExists": improved,
        "selectedFormat": if improved { "ncm4-pouw-v1" } else { session.source_format().as_str() },
        "semanticRoot": hash_hex(&best.semantic_root),
        "encodingHash": hash_hex(&best.encoding_hash),
        "sourceBytes": source_bytes,
        "fixedHeaderBytes": best.stats.fixed_header_bytes,
        "profileHeaderBytes": best.stats.profile_header_bytes,
        "bodyBytes": best.stats.body_bytes,
        "residualBytes": best.stats.residual_bytes,
        "candidateBytes": best.stats.total_bytes,
        "savedBytes": saved_bytes,
        "savedBps": if source_bytes == 0 { 0 } else { saved_bytes * 10000 / i64::from(source_bytes) },
        "decodeUnits": best.stats.decode_units,
        "attempts": session.attempts(),
        "attemptsPerSecond": if elapsed_ms(started) == 0 { 0.0 } else { session.attempts() as f64 * 1000.0 / elapsed_ms(started) as f64 },
        "generation": session.generation(),
        "elapsedMs": elapsed_ms(started),
        "threads": session.config().threads,
        "islands": session.config().islands,
        "strategy": "beam-rewrite+typed-island-lns",
        "seed": session.config().seed,
        "shardIndex": session.config().shard_index,
        "shardCount": session.config().shard_count,
        "output": args.out,
        "checkpoint": checkpoint_path,
        "stopped": control.is_stopped(),
    });
    print_report(&report, json_output)
}

fn print_ncm4_progress(progress: &Ncm4SearchProgress, json_progress: bool, elapsed: u64) {
    let rate = if elapsed == 0 {
        0.0
    } else {
        progress.attempts as f64 * 1000.0 / elapsed as f64
    };
    if json_progress {
        eprintln!(
            "{}",
            json!({
                "type": "ncm4-progress",
                "generation": progress.generation,
                "attempts": progress.attempts,
                "attemptsPerSecond": rate,
                "elapsedMs": elapsed,
                "headerBytes": progress.header_bytes,
                "bodyBytes": progress.body_bytes,
                "residualBytes": progress.residual_bytes,
                "totalBytes": progress.best_bytes,
                "decodeUnits": progress.decode_units,
                "semanticRoot": hash_hex(&progress.semantic_root),
                "exact": true,
                "witnessExists": progress.witness_exists,
                "strategy": progress.strategy,
            })
        );
    } else {
        eprintln!(
            "generation={} attempts={} rate={:.2}/s elapsed={:.3}s bytes={} (header={}, body={}, residual={}) decodeUnits={} exact=true strategy={}",
            progress.generation,
            progress.attempts,
            rate,
            elapsed as f64 / 1000.0,
            progress.best_bytes,
            progress.header_bytes,
            progress.body_bytes,
            progress.residual_bytes,
            progress.decode_units,
            progress.strategy,
        );
    }
}

fn verify(args: VerifyArgs, json_output: bool) -> CoreResult<()> {
    let task = read_task(&args.task)?;
    let result = read_result(&args.result)?;
    let report = verify_result(&task, &result)?;
    print_verification(&report, None, json_output)?;
    if !report.exact {
        return Err(Error::new(
            ErrorKind::SemanticMismatch,
            "result-semantic-mismatch",
            "Result is not an exact semantic match.",
        ));
    }
    if !report.improved {
        return Err(Error::new(
            ErrorKind::NotSmaller,
            "result-not-smaller",
            "Exact result is not strictly smaller than the incumbent.",
        ));
    }
    Ok(())
}

fn decode(args: DecodeArgs, json_output: bool) -> CoreResult<()> {
    let result = read_result(&args.result)?;
    let profile = result
        .candidate_encoding
        .get(5)
        .copied()
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Truncated,
                "candidate-header",
                "Candidate header is truncated.",
            )
        })
        .and_then(Profile::from_u8)?;
    if candidate_encoding_hash(profile, &result.candidate_encoding) != result.encoding_hash {
        return Err(Error::new(
            ErrorKind::HashMismatch,
            "result-encoding-hash",
            "Result encoding hash does not match its candidate bytes.",
        ));
    }
    let decoded = decode_candidate(&result.candidate_encoding, profile, &LimitsV1::default())?;
    let report = json!({
        "profile": profile.as_str(),
        "semanticRoot": hash_hex(&semantic_root(&decoded.semantics)),
        "encodingHash": hash_hex(&result.encoding_hash),
        "stats": decoded.stats,
        "semantics": decoded.semantics,
    });
    let bytes = serde_json::to_vec_pretty(&report).map_err(json_error)?;
    write_atomic(&args.out, &bytes)?;
    if json_output {
        print_report(&json!({"output": args.out, "report": report}), true)
    } else {
        println!("decoded={}", args.out.display());
        Ok(())
    }
}

fn benchmark(args: BenchmarkArgs, json_output: bool) -> CoreResult<()> {
    let mut paths = Vec::new();
    collect_files(&args.corpus, &mut paths)?;
    paths.sort();
    let mut rows = Vec::new();
    for path in paths {
        let Some(profile) = profile_from_path(&path) else {
            continue;
        };
        let input = read_path(&path)?;
        let limits = LimitsV1::default();
        let imported = import_asset(profile, &input, &limits)?;
        let started = Instant::now();
        let candidate = best_baseline(&imported.semantics, &limits)?;
        let pouw_elapsed = elapsed_ms(started);
        let ncm4_started = Instant::now();
        let ncm4 = deterministic_ncm4_seed(&imported, &limits)?;
        let ncm4_elapsed = elapsed_ms(ncm4_started);
        let original = imported.incumbent_encoding.len() as i64;
        let candidate_bytes = i64::from(candidate.stats.total_bytes);
        let ncm4_bytes = i64::from(ncm4.audit.ncm4_total_bytes);
        rows.push(json!({
            "file": path,
            "profile": profile.as_str(),
            "inputFormat": imported.format.as_str(),
            "incumbentBytes": original,
            "candidateBytes": candidate_bytes,
            "savedBytes": original - candidate_bytes,
            "savedBps": if original == 0 { 0 } else { (original - candidate_bytes) * 10000 / original },
            "programBytes": candidate.stats.program_bytes,
            "residualBytes": candidate.stats.residual_bytes,
            "overheadBytes": candidate.stats.overhead_bytes,
            "decodeUnits": candidate.stats.decode_units,
            "semanticRoot": hash_hex(&candidate.semantic_root),
            "exact": candidate.exact,
            "elapsedMs": pouw_elapsed,
            "selectedFormat": if ncm4_bytes < original && ncm4_bytes <= candidate_bytes {
                "ncm4-pouw-v1"
            } else if candidate_bytes < original {
                "pouw-vm-v1"
            } else {
                imported.format.as_str()
            },
            "selectedBytes": original.min(candidate_bytes).min(ncm4_bytes),
            "pouwV1": {
                "totalBytes": candidate_bytes,
                "savedBytes": original - candidate_bytes,
                "programBytes": candidate.stats.program_bytes,
                "residualBytes": candidate.stats.residual_bytes,
                "overheadBytes": candidate.stats.overhead_bytes,
                "decodeUnits": candidate.stats.decode_units,
                "exact": candidate.exact,
                "elapsedMs": pouw_elapsed,
            },
            "ncm4": {
                "fixedHeaderBytes": ncm4.audit.fixed_header_bytes,
                "profileHeaderBytes": ncm4.audit.profile_header_bytes,
                "bodyBytes": ncm4.audit.body_bytes,
                "residualBytes": ncm4.audit.residual_bytes,
                "totalBytes": ncm4.audit.ncm4_total_bytes,
                "savedBytes": original - ncm4_bytes,
                "savedBps": ncm4.audit.saved_basis_points,
                "decodeUnits": ncm4.decoded.stats.decode_units,
                "theoreticalFixedLowerBound": ncm4.audit.theoretical_fixed_lower_bound,
                "witnessExists": ncm4.audit.witness_exists,
                "recommendDeepSearch": ncm4.audit.recommend_deep_search,
                "exact": ncm4.audit.exact,
                "semanticRoot": hash_hex(&ncm4.audit.candidate_semantic_root),
                "encodingHash": hash_hex(&ncm4.decoded.encoding_hash),
                "elapsedMs": ncm4_elapsed,
            },
        }));
    }
    if rows.is_empty() {
        return Err(Error::invalid(
            "benchmark-corpus",
            "Corpus contains no recognized terrain_delta, building, or forged_item vectors.",
        ));
    }
    print_report(&json!({"vectors": rows}), json_output)
}

fn self_test(json_output: bool) -> CoreResult<()> {
    let limits = LimitsV1::default();
    let mut checks = Vec::new();

    let mut terrain = vec![0_u8; 16 + 64 * 3];
    terrain[0..4].copy_from_slice(b"NCBK");
    terrain[4] = 1;
    terrain[6..8].copy_from_slice(&16_u16.to_le_bytes());
    terrain[8..10].copy_from_slice(&64_u16.to_le_bytes());
    for index in 0..16_usize {
        terrain[16 + index * 3] = index as u8;
    }
    checks.push(run_self_test_vector(
        Profile::TerrainDelta,
        &terrain,
        &limits,
    )?);

    let building = [1_u8, 8, 8, 8, 1, 1, 7, 0, 0, 0, 7, 3, 7];
    checks.push(run_self_test_vector(Profile::Building, &building, &limits)?);

    let forged = hex_bytes("f000c0d8000108310518720928b0450081024000")?;
    checks.push(run_self_test_vector(Profile::ForgedItem, &forged, &limits)?);

    let report = json!({
        "ok": true,
        "softwareVersion": env!("CARGO_PKG_VERSION"),
        "commit": env!("NICECHUNK_GIT_COMMIT"),
        "protocolVersion": PROTOCOL_VERSION,
        "vmVersion": VM_VERSION,
        "costModelVersion": COST_MODEL_VERSION,
        "checks": checks,
    });
    print_report(&report, json_output)
}

fn run_self_test_vector(profile: Profile, input: &[u8], limits: &LimitsV1) -> CoreResult<Value> {
    let imported = import_asset(profile, input, limits)?;
    let candidate = best_baseline(&imported.semantics, limits)?;
    let task = TaskV1::create(
        imported,
        format!("self-test:{}", profile.as_str()),
        limits.clone(),
        None,
    )?;
    let result = ResultV1::create(&task, candidate.encoding, None, None)?;
    let report = verify_result(&task, &result)?;
    if !report.exact || report.mismatch_count != 0 {
        return Err(Error::new(
            ErrorKind::Internal,
            "self-test-exact",
            "Built-in profile vector failed exact verification.",
        ));
    }
    let task_round_trip = TaskV1::from_bytes(&task.to_bytes()?)?;
    let result_round_trip = ResultV1::from_bytes(&result.to_bytes()?)?;
    if task_round_trip != task || result_round_trip != result {
        return Err(Error::new(
            ErrorKind::Internal,
            "self-test-round-trip",
            "Task or Result binary round-trip changed its value.",
        ));
    }
    Ok(json!({
        "profile": profile.as_str(),
        "exact": report.exact,
        "mismatchCount": report.mismatch_count,
        "incumbentBytes": report.incumbent_bytes,
        "candidateBytes": report.candidate_bytes,
        "decodeUnits": report.vm_stats.decode_units,
        "semanticRoot": hash_hex(&report.candidate_semantic_root),
    }))
}

fn read_task(path: &Path) -> CoreResult<TaskV1> {
    TaskV1::from_bytes(&read_path(path)?)
}

fn read_result(path: &Path) -> CoreResult<ResultV1> {
    ResultV1::from_bytes(&read_path(path)?)
}

fn read_path(path: &Path) -> CoreResult<Vec<u8>> {
    if path == Path::new("-") {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes).map_err(io_error)?;
        return Ok(bytes);
    }
    fs::read(path).map_err(io_error)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(io_error)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(io_error)?;
    temporary.write_all(bytes).map_err(io_error)?;
    temporary.flush().map_err(io_error)?;
    temporary.as_file().sync_all().map_err(io_error)?;
    temporary
        .persist(path)
        .map_err(|error| io_error(error.error))?;
    Ok(())
}

fn print_verification(
    report: &VerificationReportV1,
    output: Option<&Path>,
    json_output: bool,
) -> CoreResult<()> {
    print_report(&verification_json(report, output), json_output)
}

fn verification_json(report: &VerificationReportV1, output: Option<&Path>) -> Value {
    json!({
        "accepted": report.accepted,
        "exact": report.exact,
        "improved": report.improved,
        "mismatchCount": report.mismatch_count,
        "taskId": hash_hex(&report.task_id),
        "targetSemanticRoot": hash_hex(&report.target_semantic_root),
        "candidateSemanticRoot": hash_hex(&report.candidate_semantic_root),
        "incumbentEncodingHash": hash_hex(&report.incumbent_encoding_hash),
        "candidateEncodingHash": hash_hex(&report.candidate_encoding_hash),
        "incumbentBytes": report.incumbent_bytes,
        "programBytes": report.vm_stats.program_bytes,
        "residualBytes": report.vm_stats.residual_bytes,
        "overheadBytes": report.vm_stats.overhead_bytes,
        "candidateBytes": report.candidate_bytes,
        "savedBytes": report.saved_bytes,
        "savedBps": report.saved_bps,
        "decodeUnits": report.vm_stats.decode_units,
        "writes": report.vm_stats.writes,
        "commands": report.vm_stats.commands,
        "patches": report.vm_stats.patches,
        "output": output,
    })
}

fn print_report(value: &Value, json_output: bool) -> CoreResult<()> {
    if json_output {
        println!("{}", serde_json::to_string(value).map_err(json_error)?);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).map_err(json_error)?
        );
    }
    Ok(())
}

fn merge_json(mut left: Value, right: Value) -> Value {
    if let (Some(left), Some(right)) = (left.as_object_mut(), right.as_object()) {
        for (key, value) in right {
            left.insert(key.clone(), value.clone());
        }
    }
    left
}

fn parse_threads(value: &str) -> CoreResult<u16> {
    if value == "auto" {
        return Ok(std::thread::available_parallelism()
            .map(|value| value.get().saturating_sub(1).max(1))
            .unwrap_or(1)
            .min(usize::from(u16::MAX)) as u16);
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            Error::invalid(
                "threads",
                "--threads must be auto or an integer in 1..=65535.",
            )
        })
}

fn resolve_profile(
    path: &Path,
    input: &[u8],
    requested: Option<Profile>,
    limits: &LimitsV1,
) -> CoreResult<Profile> {
    if let Some(profile) = requested {
        return Ok(profile);
    }
    match detect_format(input) {
        DetectedFormat::ChunkBrokenV1 => Ok(Profile::TerrainDelta),
        DetectedFormat::Ncm3V1 => Ok(Profile::Building),
        DetectedFormat::Ncf1V15 => Ok(Profile::ForgedItem),
        DetectedFormat::Ncm4PouwV1 => Ok(decode_ncm4(input, limits)?.profile),
        DetectedFormat::PouwVmV1 => input
            .get(5)
            .copied()
            .ok_or_else(|| Error::new(ErrorKind::Truncated, "candidate-header", "Candidate header is truncated."))
            .and_then(Profile::from_u8),
        DetectedFormat::Unknown => profile_from_path(path).ok_or_else(|| {
            Error::invalid(
                "profile-required",
                "Input format is ambiguous; pass --profile terrain_delta, building, or forged_item.",
            )
        }),
    }
}

fn parse_duration(value: &str) -> CoreResult<u64> {
    let value = value.trim();
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3_600_000)
    } else {
        (value, 1_000)
    };
    number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| {
            Error::invalid(
                "time-limit",
                "Time limit must be a positive integer with ms, s, m, or h suffix.",
            )
        })
}

fn collect_files(path: &Path, output: &mut Vec<PathBuf>) -> CoreResult<()> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    if metadata.is_file() {
        output.push(path.to_owned());
        return Ok(());
    }
    let mut entries = fs::read_dir(path)
        .map_err(io_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(io_error)?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_files(&entry.path(), output)?;
    }
    Ok(())
}

fn profile_from_path(path: &Path) -> Option<Profile> {
    let value = path.to_string_lossy().to_ascii_lowercase();
    if value.contains("terrain_delta") || value.ends_with(".ncbk") {
        Some(Profile::TerrainDelta)
    } else if value.contains("forged_item") || value.ends_with(".ncf1") {
        Some(Profile::ForgedItem)
    } else if value.contains("building") || value.ends_with(".ncm3") {
        Some(Profile::Building)
    } else {
        None
    }
}

fn hex_bytes(value: &str) -> CoreResult<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(Error::invalid("hex", "Hex input has an odd length."));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair =
                std::str::from_utf8(pair).map_err(|_| Error::invalid("hex", "Invalid hex."))?;
            u8::from_str_radix(pair, 16).map_err(|_| Error::invalid("hex", "Invalid hex."))
        })
        .collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn io_error(error: io::Error) -> Error {
    Error::new(ErrorKind::InvalidInput, "io", error.to_string())
}

fn json_error(error: serde_json::Error) -> Error {
    Error::new(ErrorKind::Internal, "json", error.to_string())
}

fn exit_code(kind: ErrorKind) -> u8 {
    match kind {
        ErrorKind::InvalidInput
        | ErrorKind::UnsupportedVersion
        | ErrorKind::NonCanonical
        | ErrorKind::Truncated
        | ErrorKind::TrailingData
        | ErrorKind::UnknownOpcode
        | ErrorKind::OutOfBounds => 2,
        ErrorKind::ResourceLimit | ErrorKind::ArithmeticOverflow => 3,
        ErrorKind::HashMismatch | ErrorKind::SemanticMismatch | ErrorKind::NotSmaller => 4,
        ErrorKind::Internal => 70,
    }
}
