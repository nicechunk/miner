#![forbid(unsafe_code)]

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use pouw_core::{
    candidate_encoding_hash, decode_candidate, encoding_hash, hash_hex, import_asset,
    semantic_root, verify_result, Error, LimitsV1, Profile, ResultV1, SearchMetadataV1, TaskV1,
    VerificationReportV1, COST_MODEL_VERSION, PROTOCOL_VERSION, SOFTWARE_VERSION, VM_VERSION,
};
use pouw_search::{best_baseline, mine, resume, CheckpointV1, SearchConfig, SearchControl};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn version_json() -> String {
    json!({
        "softwareVersion": SOFTWARE_VERSION,
        "protocolVersion": PROTOCOL_VERSION,
        "vmVersion": VM_VERSION,
        "costModelVersion": COST_MODEL_VERSION,
    })
    .to_string()
}

#[wasm_bindgen]
pub fn inspect_json(profile: &str, input: &[u8]) -> Result<String, JsValue> {
    let profile = parse_profile(profile)?;
    let limits = LimitsV1::default();
    let imported = import_asset(profile, input, &limits).map_err(js_error)?;
    Ok(json!({
        "profile": imported.profile.as_str(),
        "format": imported.format.as_str(),
        "incumbentBytes": imported.incumbent_encoding.len(),
        "semanticRoot": hash_hex(&semantic_root(&imported.semantics)),
        "encodingHash": hash_hex(&encoding_hash(imported.profile, imported.format, &imported.incumbent_encoding)),
        "voxelCount": imported.semantics.voxel_count(),
        "semantics": imported.semantics,
    }).to_string())
}

#[wasm_bindgen]
pub fn baseline_json(profile: &str, input: &[u8]) -> Result<String, JsValue> {
    let profile = parse_profile(profile)?;
    let limits = LimitsV1::default();
    let imported = import_asset(profile, input, &limits).map_err(js_error)?;
    let candidate = best_baseline(&imported.semantics, &limits).map_err(js_error)?;
    let task = TaskV1::create(imported, "browser:local", limits, None).map_err(js_error)?;
    let result = ResultV1::create(
        &task,
        candidate.encoding,
        None,
        Some(SearchMetadataV1 {
            algorithm: "deterministic-baseline-v1".into(),
            attempts: 1,
            elapsed_ms: 0,
            seed: 0,
            threads: 1,
        }),
    )
    .map_err(js_error)?;
    response_json(&task, &result, None, 1, 0, 0)
}

#[wasm_bindgen]
pub fn mine_slice_json(
    profile: &str,
    input: &[u8],
    seed: u64,
    generations: u32,
    population: u32,
) -> Result<String, JsValue> {
    let profile = parse_profile(profile)?;
    let limits = LimitsV1::default();
    let imported = import_asset(profile, input, &limits).map_err(js_error)?;
    let task = TaskV1::create(imported, "browser:local", limits, None).map_err(js_error)?;
    let config = browser_config(seed, generations, population, &task)?;
    let outcome = mine(&task, &config, &SearchControl::default(), |_| {}).map_err(js_error)?;
    response_json(
        &task,
        &outcome.result,
        Some(&outcome.checkpoint),
        outcome.attempts,
        outcome.elapsed_ms,
        outcome.generations,
    )
}

#[wasm_bindgen]
pub fn resume_slice_json(checkpoint_bytes: &[u8]) -> Result<String, JsValue> {
    let checkpoint = CheckpointV1::from_bytes(checkpoint_bytes).map_err(js_error)?;
    let task = TaskV1::from_bytes(&checkpoint.task_bytes).map_err(js_error)?;
    checkpoint.validate_for_task(&task).map_err(js_error)?;
    let outcome =
        resume(&task, &checkpoint, &SearchControl::default(), |_| {}).map_err(js_error)?;
    response_json(
        &task,
        &outcome.result,
        Some(&outcome.checkpoint),
        outcome.attempts,
        outcome.elapsed_ms,
        outcome.generations,
    )
}

#[wasm_bindgen]
pub fn verify_local_json(
    profile: &str,
    incumbent: &[u8],
    candidate: &[u8],
) -> Result<String, JsValue> {
    let profile = parse_profile(profile)?;
    let limits = LimitsV1::default();
    let imported = import_asset(profile, incumbent, &limits).map_err(js_error)?;
    let task = TaskV1::create(imported, "browser:local", limits, None).map_err(js_error)?;
    let result = ResultV1::create(&task, candidate.to_vec(), None, None).map_err(js_error)?;
    let report = verify_result(&task, &result).map_err(js_error)?;
    Ok(report_json(&report).to_string())
}

#[wasm_bindgen]
pub fn decode_candidate_json(candidate: &[u8]) -> Result<String, JsValue> {
    let profile = candidate
        .get(5)
        .copied()
        .ok_or_else(|| JsValue::from_str("candidate-header: candidate is truncated"))
        .and_then(|value| Profile::from_u8(value).map_err(js_error))?;
    let decoded = decode_candidate(candidate, profile, &LimitsV1::default()).map_err(js_error)?;
    Ok(json!({
        "profile": profile.as_str(),
        "semanticRoot": hash_hex(&semantic_root(&decoded.semantics)),
        "encodingHash": hash_hex(&candidate_encoding_hash(profile, candidate)),
        "stats": decoded.stats,
        "semantics": decoded.semantics,
    })
    .to_string())
}

fn response_json(
    task: &TaskV1,
    result: &ResultV1,
    checkpoint: Option<&CheckpointV1>,
    attempts: u64,
    elapsed_ms: u64,
    generations: u32,
) -> Result<String, JsValue> {
    let report = verify_result(task, result).map_err(js_error)?;
    let mut value = report_json(&report);
    let object = value
        .as_object_mut()
        .ok_or_else(|| JsValue::from_str("internal: report is not an object"))?;
    object.insert(
        "candidateBase64".into(),
        Value::String(STANDARD.encode(&result.candidate_encoding)),
    );
    object.insert(
        "resultBase64".into(),
        Value::String(STANDARD.encode(result.to_bytes().map_err(js_error)?)),
    );
    object.insert(
        "taskBase64".into(),
        Value::String(STANDARD.encode(task.to_bytes().map_err(js_error)?)),
    );
    object.insert("attempts".into(), json!(attempts));
    object.insert("elapsedMs".into(), json!(elapsed_ms));
    object.insert("generations".into(), json!(generations));
    if let Some(checkpoint) = checkpoint {
        object.insert(
            "checkpointBase64".into(),
            Value::String(STANDARD.encode(checkpoint.to_bytes().map_err(js_error)?)),
        );
    }
    Ok(value.to_string())
}

fn report_json(report: &VerificationReportV1) -> Value {
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
    })
}

fn browser_config(
    seed: u64,
    generations: u32,
    population: u32,
    task: &TaskV1,
) -> Result<SearchConfig, JsValue> {
    let config = SearchConfig {
        seed,
        threads: 1,
        islands: 1,
        population: population.clamp(4, 512),
        generations: generations.clamp(1, 64),
        epoch_generations: 1,
        elite_count: (population / 16).clamp(1, 16) as u16,
        tournament_size: 3,
        max_attempts: None,
        time_limit_ms: None,
        memory_limit_bytes: 64 * 1024 * 1024,
    };
    config.validate(&task.limits).map_err(js_error)?;
    Ok(config)
}

fn parse_profile(value: &str) -> Result<Profile, JsValue> {
    value.parse().map_err(js_error)
}

fn js_error(error: Error) -> JsValue {
    JsValue::from_str(&format!("{}: {}", error.code, error.message))
}
