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

    let mined_report = successful_json(&[
        "--json",
        "mine",
        source,
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
    assert_eq!(mined_report["exact"], true);
    assert_eq!(mined_report["threads"], 2);
    assert_eq!(mined_report["islands"], 2);
    assert!(mined.exists());
    assert!(checkpoint.exists());

    fs::remove_dir_all(&temporary).unwrap();
}
