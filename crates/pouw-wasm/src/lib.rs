#![forbid(unsafe_code)]

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use pouw_core::{
    candidate_encoding_hash, decode_candidate, decode_ncm4, deterministic_ncm4_seed, encoding_hash,
    hash_hex, import_asset, semantic_root, verify_result, Error, IncumbentFormat, LimitsV1,
    Profile, ResultV1, SearchMetadataV1, TaskV1, VerificationReportV1, COST_MODEL_VERSION,
    NCM4_VERSION, PROTOCOL_VERSION, SOFTWARE_VERSION, VM_VERSION,
};
use pouw_search::{
    best_baseline, mine, resume, CheckpointV1, Ncm4SearchCheckpoint, Ncm4SearchSession,
    SearchConfig, SearchControl,
};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn version_json() -> String {
    json!({
        "softwareVersion": SOFTWARE_VERSION,
        "protocolVersion": PROTOCOL_VERSION,
        "vmVersion": VM_VERSION,
        "costModelVersion": COST_MODEL_VERSION,
        "ncm4Version": NCM4_VERSION,
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
pub fn ncm4_analyze_json(profile: &str, input: &[u8]) -> Result<String, JsValue> {
    let profile = parse_profile(profile)?;
    let limits = LimitsV1::default();
    let imported = import_asset(profile, input, &limits).map_err(js_error)?;
    if imported.format == IncumbentFormat::Ncm4PouwV1 {
        let decoded = decode_ncm4(&imported.incumbent_encoding, &limits).map_err(js_error)?;
        return Ok(json!({
            "inputFormat": imported.format.as_str(),
            "profile": profile.as_str(),
            "sourceBytes": imported.incumbent_encoding.len(),
            "ncm4TotalBytes": decoded.stats.total_bytes,
            "fixedHeaderBytes": decoded.stats.fixed_header_bytes,
            "profileHeaderBytes": decoded.stats.profile_header_bytes,
            "bodyBytes": decoded.stats.body_bytes,
            "residualBytes": decoded.stats.residual_bytes,
            "patches": decoded.stats.patches,
            "semanticRoot": hash_hex(&decoded.semantic_root),
            "candidateSemanticRoot": hash_hex(&decoded.semantic_root),
            "encodingHash": hash_hex(&decoded.encoding_hash),
            "decodeUnits": decoded.stats.decode_units,
            "exact": true,
            "witnessExists": false,
            "recommendDeepSearch": profile == Profile::Building,
            "selectedFormat": imported.format.as_str(),
            "candidateBase64": STANDARD.encode(&decoded.raw_encoding),
            "semantics": decoded.semantics,
        })
        .to_string());
    }
    let seed = deterministic_ncm4_seed(&imported, &limits).map_err(js_error)?;
    Ok(json!({
        "inputFormat": seed.audit.source_format,
        "profile": seed.audit.profile.as_str(),
        "sourceBytes": seed.audit.source_bytes,
        "fixedHeaderBytes": seed.audit.fixed_header_bytes,
        "profileHeaderBytes": seed.audit.profile_header_bytes,
        "bodyBytes": seed.audit.body_bytes,
        "residualBytes": seed.audit.residual_bytes,
        "patches": seed.decoded.stats.patches,
        "ncm4TotalBytes": seed.audit.ncm4_total_bytes,
        "theoreticalFixedLowerBound": seed.audit.theoretical_fixed_lower_bound,
        "savedBytes": seed.audit.saved_bytes,
        "savedBps": seed.audit.saved_basis_points,
        "semanticRoot": hash_hex(&seed.audit.semantic_root),
        "candidateSemanticRoot": hash_hex(&seed.audit.candidate_semantic_root),
        "encodingHash": hash_hex(&seed.decoded.encoding_hash),
        "decodeUnits": seed.decoded.stats.decode_units,
        "exact": seed.audit.exact,
        "witnessExists": seed.audit.witness_exists,
        "recommendDeepSearch": seed.audit.recommend_deep_search,
        "selectedFormat": seed.audit.selected_format,
        "candidateBase64": STANDARD.encode(&seed.encoding),
        "semantics": seed.decoded.semantics,
    })
    .to_string())
}

#[wasm_bindgen]
pub fn decode_ncm4_json(input: &[u8]) -> Result<String, JsValue> {
    let decoded = decode_ncm4(input, &LimitsV1::default()).map_err(js_error)?;
    Ok(json!({
        "format": "ncm4-pouw-v1",
        "profile": decoded.profile.as_str(),
        "semanticRoot": hash_hex(&decoded.semantic_root),
        "encodingHash": hash_hex(&decoded.encoding_hash),
        "stats": decoded.stats,
        "semantics": decoded.semantics,
    })
    .to_string())
}

#[wasm_bindgen]
pub fn verify_ncm4_json(profile: &str, source: &[u8], candidate: &[u8]) -> Result<String, JsValue> {
    let profile = parse_profile(profile)?;
    let limits = LimitsV1::default();
    let source = import_asset(profile, source, &limits).map_err(js_error)?;
    let candidate = decode_ncm4(candidate, &limits).map_err(js_error)?;
    let target_root = semantic_root(&source.semantics);
    let mismatch_count = source.semantics.mismatch_count(&candidate.semantics);
    let exact = candidate.profile == profile
        && candidate.semantic_root == target_root
        && mismatch_count == 0;
    let improved = exact && candidate.raw_encoding.len() < source.incumbent_encoding.len();
    Ok(json!({
        "accepted": improved,
        "exact": exact,
        "improved": improved,
        "mismatchCount": mismatch_count,
        "targetSemanticRoot": hash_hex(&target_root),
        "candidateSemanticRoot": hash_hex(&candidate.semantic_root),
        "candidateEncodingHash": hash_hex(&candidate.encoding_hash),
        "sourceBytes": source.incumbent_encoding.len(),
        "candidateBytes": candidate.raw_encoding.len(),
        "savedBytes": source.incumbent_encoding.len() as i64 - candidate.raw_encoding.len() as i64,
        "stats": candidate.stats,
        "selectedFormat": if improved { "ncm4-pouw-v1" } else { source.format.as_str() },
    })
    .to_string())
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
pub fn migrate_checkpoint_elite(
    checkpoint_bytes: &[u8],
    external_checkpoint_bytes: &[u8],
) -> Result<Vec<u8>, JsValue> {
    let checkpoint = CheckpointV1::from_bytes(checkpoint_bytes).map_err(js_error)?;
    let external = CheckpointV1::from_bytes(external_checkpoint_bytes).map_err(js_error)?;
    checkpoint
        .migrate_verified_elite(&external)
        .and_then(|merged| merged.to_bytes())
        .map_err(js_error)
}

#[wasm_bindgen]
pub struct BrowserNcm4Session {
    inner: Ncm4SearchSession,
}

#[wasm_bindgen]
impl BrowserNcm4Session {
    #[wasm_bindgen(constructor)]
    pub fn new(
        profile: &str,
        input: &[u8],
        seed: u64,
        population: u32,
        shard_index: u32,
        shard_count: u32,
    ) -> Result<BrowserNcm4Session, JsValue> {
        let profile = parse_profile(profile)?;
        let limits = LimitsV1::default();
        let imported = import_asset(profile, input, &limits).map_err(js_error)?;
        let population = population.clamp(4, 512);
        let config = SearchConfig {
            seed,
            threads: 1,
            islands: 1,
            population,
            generations: 1,
            epoch_generations: 1,
            elite_count: (population / 16).clamp(1, 16) as u16,
            tournament_size: 3,
            max_attempts: None,
            time_limit_ms: None,
            memory_limit_bytes: 64 * 1024 * 1024,
            shard_index,
            shard_count,
        };
        Ok(Self {
            inner: Ncm4SearchSession::new(imported, config).map_err(js_error)?,
        })
    }

    #[wasm_bindgen(js_name = fromCheckpoint)]
    pub fn from_checkpoint(bytes: &[u8]) -> Result<BrowserNcm4Session, JsValue> {
        let checkpoint = Ncm4SearchCheckpoint::from_bytes(bytes).map_err(js_error)?;
        Ok(Self {
            inner: Ncm4SearchSession::from_checkpoint(&checkpoint).map_err(js_error)?,
        })
    }

    #[wasm_bindgen(js_name = stepJson)]
    pub fn step_json(&mut self, generations: u32) -> Result<String, JsValue> {
        let outcome = self
            .inner
            .step(generations.clamp(1, 16), |_| {})
            .map_err(js_error)?;
        ncm4_search_response(&self.inner, &outcome.checkpoint)
    }

    #[wasm_bindgen(js_name = injectCheckpoint)]
    pub fn inject_checkpoint(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        let checkpoint = Ncm4SearchCheckpoint::from_bytes(bytes).map_err(js_error)?;
        self.inner.inject_checkpoint(&checkpoint).map_err(js_error)
    }

    #[wasm_bindgen(js_name = checkpointBytes)]
    pub fn checkpoint_bytes(&self) -> Result<Vec<u8>, JsValue> {
        self.inner
            .checkpoint()
            .and_then(|checkpoint| checkpoint.to_bytes())
            .map_err(js_error)
    }
}

fn ncm4_search_response(
    session: &Ncm4SearchSession,
    checkpoint: &Ncm4SearchCheckpoint,
) -> Result<String, JsValue> {
    let best = session.best();
    let source_bytes = session.source_bytes();
    let candidate_bytes = best.stats.total_bytes;
    let saved_bytes = i64::from(source_bytes) - i64::from(candidate_bytes);
    let improved = candidate_bytes < source_bytes;
    let source_format = session.source_format();
    Ok(json!({
        "format": "ncm4-pouw-v1",
        "accepted": improved,
        "exact": best.exact,
        "improved": improved,
        "witnessExists": improved,
        "mismatchCount": 0,
        "targetSemanticRoot": hash_hex(&best.semantic_root),
        "candidateSemanticRoot": hash_hex(&best.semantic_root),
        "candidateEncodingHash": hash_hex(&best.encoding_hash),
        "incumbentBytes": source_bytes,
        "sourceBytes": source_bytes,
        "sourceEncodingHash": hash_hex(&session.source_encoding_hash()),
        "fixedHeaderBytes": best.stats.fixed_header_bytes,
        "profileHeaderBytes": best.stats.profile_header_bytes,
        "programBytes": best.stats.body_bytes,
        "bodyBytes": best.stats.body_bytes,
        "residualBytes": best.stats.residual_bytes,
        "overheadBytes": best.stats.fixed_header_bytes + best.stats.profile_header_bytes,
        "candidateBytes": candidate_bytes,
        "savedBytes": saved_bytes,
        "savedBps": if source_bytes == 0 { 0 } else { saved_bytes * 10000 / i64::from(source_bytes) },
        "decodeUnits": best.stats.decode_units,
        "writes": best.stats.writes,
        "commands": best.stats.commands,
        "patches": best.stats.patches,
        "attempts": session.attempts(),
        "elapsedMs": 0,
        "generations": session.generation(),
        "generation": session.generation(),
        "strategy": "beam-rewrite+typed-island-lns",
        "selectedFormat": if improved { "ncm4-pouw-v1" } else { source_format.as_str() },
        "candidateBase64": STANDARD.encode(&best.encoding),
        "checkpointBase64": STANDARD.encode(checkpoint.to_bytes().map_err(js_error)?),
    }).to_string())
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
        shard_index: 0,
        shard_count: 1,
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
