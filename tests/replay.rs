//! Strict replay gate: every recorded baseline must reproduce; skipped when its cache is absent.

use std::path::Path;
use std::process::Command;

#[test]
fn baselines_replay_strictly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut names: Vec<_> = std::fs::read_dir(root.join("evals/results"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".base.json"))
        .collect();
    names.sort();
    let mut ran = 0;
    for baseline in names {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
        let scenario = doc["scenario"].as_str().unwrap();
        let control = doc["control"].as_bool().unwrap_or(false);
        let cache = root.join("evals/cache").join(format!(
            "{scenario}{}.json",
            if control { ".control" } else { "" }
        ));
        if !cache.exists() {
            eprintln!("skip {}: no cache", baseline.display());
            continue;
        }
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_eval"));
        cmd.arg(
            root.join("evals/scenarios")
                .join(format!("{scenario}.json")),
        )
        .arg("--strict")
        .arg("--baseline")
        .arg(&baseline);
        if control {
            cmd.arg("--control");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{} failed strict replay:\n{}\n{}",
            baseline.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        ran += 1;
    }
    eprintln!("replayed {ran} baselines");
}
