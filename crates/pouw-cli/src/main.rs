#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand};
use pouw_core::{
    candidate_encoding_hash, decode_candidate, encoding_hash, hash_hex, import_asset,
    import_incumbent, semantic_root, verify_result, Error, ErrorKind, LimitsV1, Profile,
    Result as CoreResult, ResultV1, SearchMetadataV1, TaskV1, VerificationReportV1,
    COST_MODEL_VERSION, PROTOCOL_VERSION, VM_VERSION,
};
use pouw_search::{
    best_baseline, mine, resume, CheckpointV1, SearchConfig, SearchControl, SearchProgress,
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
    about = "NiceChunk Proof of Useful Work Miner v1",
    long_about = "Deterministically compress, search, decode, and independently verify exact NiceChunk voxel assets."
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
    Task(TaskArgs),
    Baseline(BaselineArgs),
    Mine(MineArgs),
    Verify(VerifyArgs),
    Decode(DecodeArgs),
    Benchmark(BenchmarkArgs),
    SelfTest,
}

#[derive(Args, Debug)]
struct InspectArgs {
    input: PathBuf,
    #[arg(long)]
    profile: Profile,
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
    #[arg(long, required_unless_present = "resume", conflicts_with = "resume")]
    task: Option<PathBuf>,
    #[arg(long, required_unless_present = "task", conflicts_with = "task")]
    resume: Option<PathBuf>,
    #[arg(long, default_value = "auto")]
    threads: String,
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
    #[arg(long)]
    checkpoint: Option<PathBuf>,
    #[arg(long, default_value = "result.ncpow")]
    out: PathBuf,
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
    #[arg(long)]
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
        Command::Task(args) => match args.command {
            TaskCommand::Create(args) => create_task(args, cli.json),
        },
        Command::Baseline(args) => baseline(args, cli.json),
        Command::Mine(args) => mine_command(args, cli.json, cli.json_progress),
        Command::Verify(args) => verify(args, cli.json),
        Command::Decode(args) => decode(args, cli.json),
        Command::Benchmark(args) => benchmark(args, cli.json),
        Command::SelfTest => self_test(cli.json),
    }
}

fn inspect(args: InspectArgs, json_output: bool) -> CoreResult<()> {
    let input = read_path(&args.input)?;
    let limits = LimitsV1::default();
    let imported = import_asset(args.profile, &input, &limits)?;
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
            islands: threads.clamp(1, 8),
            population: args.population,
            generations: args.generations,
            elite_count: (args.population / 16).clamp(1, u32::from(u16::MAX)) as u16,
            tournament_size: args.population.clamp(2, 3) as u8,
            max_attempts: args.max_attempts,
            time_limit_ms: args.time_limit.as_deref().map(parse_duration).transpose()?,
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
        let elapsed = elapsed_ms(started);
        let original = imported.incumbent_encoding.len() as i64;
        let candidate_bytes = i64::from(candidate.stats.total_bytes);
        rows.push(json!({
            "file": path,
            "profile": profile.as_str(),
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
            "elapsedMs": elapsed,
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
