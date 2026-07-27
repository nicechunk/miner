use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=NICECHUNK_GIT_COMMIT");
    let commit = std::env::var("NICECHUNK_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=NICECHUNK_GIT_COMMIT={commit}");
}
