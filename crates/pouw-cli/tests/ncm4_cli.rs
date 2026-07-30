use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_nicechunk-miner")
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn run(arguments: &[&str]) -> Output {
    Command::new(binary())
        .args(arguments)
        .current_dir(repository_root())
        .output()
        .unwrap()
}

fn successful_json(arguments: &[&str]) -> Value {
    let output = run(arguments);
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn ncm4_cli_analyzes_encodes_decodes_verifies_and_mines() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temporary = std::env::temp_dir().join(format!("nicechunk-ncm4-cli-{nonce}"));
    fs::create_dir(&temporary).unwrap();
    let candidate = temporary.join("cottage.nc4p");
    let decoded = temporary.join("decoded.json");
    let mined = temporary.join("mined.nc4p");
    let checkpoint = temporary.join("state.nc4s.chk");
    let source = "test-vectors/building/complex-cottage.ncm3";
    let direct_ncm = temporary.join("pasted-building.ncm");
    fs::copy(repository_root().join(source), &direct_ncm).unwrap();

    let analysis = successful_json(&["--json", "ncm4", "analyze", source]);
    assert_eq!(analysis["exact"], true);
    assert_eq!(analysis["witnessExists"], true);
    assert_eq!(analysis["sourceBytes"], 64);
    assert_eq!(analysis["ncm4TotalBytes"], 57);

    let encoded = successful_json(&[
        "--json",
        "ncm4",
        "encode",
        source,
        "--out",
        candidate.to_str().unwrap(),
    ]);
    assert_eq!(encoded["ncm4TotalBytes"], 57);
    assert_eq!(fs::metadata(&candidate).unwrap().len(), 57);

    let decoded_report = successful_json(&[
        "--json",
        "ncm4",
        "decode",
        candidate.to_str().unwrap(),
        "--out",
        decoded.to_str().unwrap(),
    ]);
    assert_eq!(decoded_report["report"]["profile"], "building");
    assert_eq!(decoded_report["report"]["stats"]["totalBytes"], 57);
    assert!(decoded.exists());

    let verified = successful_json(&[
        "--json",
        "ncm4",
        "verify",
        "--source",
        source,
        "--candidate",
        candidate.to_str().unwrap(),
    ]);
    assert_eq!(verified["exact"], true);
    assert_eq!(verified["improved"], true);
    assert_eq!(verified["mismatchCount"], 0);

    let mined_output = run(&[
        "--json",
        "mine",
        direct_ncm.to_str().unwrap(),
        "--threads",
        "2",
        "--islands",
        "2",
        "--population",
        "4",
        "--generations",
        "1",
        "--seed",
        "123",
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--out",
        mined.to_str().unwrap(),
    ]);
    assert!(
        mined_output.status.success(),
        "direct NCM search failed: {}",
        String::from_utf8_lossy(&mined_output.stderr)
    );
    let mined_report: Value = serde_json::from_slice(&mined_output.stdout).unwrap();
    let progress = String::from_utf8_lossy(&mined_output.stderr);
    assert_eq!(mined_report["exact"], true);
    assert_eq!(mined_report["improved"], true);
    assert_eq!(mined_report["threads"], 2);
    assert_eq!(mined_report["islands"], 2);
    assert_eq!(mined_report["sourceFormat"], "ncm3-v1");
    assert_eq!(mined_report["mismatchCount"], 0);
    assert_eq!(mined_report["evaluator"]["active"], "cpu");
    assert_eq!(mined_report["evaluator"]["requested"], "auto");
    assert_eq!(mined_report["stopReason"], "generation-limit");
    assert!(mined_report["savedBytes"].as_i64().unwrap() > 0);
    assert!(progress.contains("status=starting"));
    assert!(progress.contains("threads=2"));
    assert!(progress.contains("status=improved"));
    assert!(progress.contains("sourceBytes=64"));
    assert!(progress.contains("candidateBytes=57"));
    assert!(progress.contains("savedPercent="));
    assert!(progress.contains("semanticRoot="));
    assert!(progress.contains("exact=true"));
    assert!(progress.contains("status=complete"));
    assert!(mined.exists());
    assert!(checkpoint.exists());

    fs::remove_dir_all(&temporary).unwrap();
}

#[test]
fn gpu_info_is_machine_readable_without_requiring_a_gpu() {
    let report = successful_json(&["--json", "gpu-info"]);
    assert!(report["cudaCompiled"].is_boolean());
    assert!(report["available"].is_boolean());
    assert!(report["devices"].is_array());
}
