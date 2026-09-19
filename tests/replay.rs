//! Strict replay gate: every recorded baseline must reproduce; skipped when its cache is absent.

use std::path::Path;
use std::process::Command;

#[test]
fn baselines_replay_strictly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut names: Vec<_> = std::fs::read_dir(root.join("evals/results"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.to_string_lossy().ends_with(".base.json") || p.to_string_lossy().ends_with(".v8.json")
        })
        .collect();
    names.sort();
    let mut ran = 0;
    for baseline in names {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
        let scenario = doc["scenario"].as_str().unwrap();
        let control = doc["control"].as_bool().unwrap_or(false);
        if !control && doc["harness_version"] != 8 {
            eprintln!(
                "historical harness baseline {}: prompts intentionally replaced; use v8 recordings",
                baseline.display()
            );
            continue;
        }
        let cache = root.join("evals/cache").join(format!(
            "{scenario}{}.json",
            if control { ".control" } else { ".v8" }
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
        if let Some(judge) = doc["judge_model"].as_str().filter(|j| !j.is_empty()) {
            cmd.env("JUDGE_MODEL", judge);
        }
        let audit = std::env::temp_dir().join(morpho::ids::IdGen::random().next("morpho-replay"));
        if !control {
            cmd.arg("--audit-dir").arg(&audit);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{} failed strict replay:\n{}\n{}",
            baseline.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !control {
            let replay: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(audit.join("result.json")).unwrap())
                    .unwrap();
            assert_eq!(replay["metrics"]["cache_misses"], 0);
            let replies: Vec<_> = doc["replies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["reply"].clone())
                .collect();
            assert_eq!(
                replay["replies"],
                serde_json::json!(replies),
                "reply mismatch: {scenario}"
            );
            for (key, expected) in doc["metrics"].as_object().unwrap() {
                if [
                    "seconds",
                    "cache_misses",
                    "provider_prompt_tokens",
                    "provider_completion_tokens",
                    "usage_reported_calls",
                ]
                .contains(&key.as_str())
                {
                    continue;
                }
                assert_eq!(
                    &replay["metrics"][key], expected,
                    "metric {key}: {scenario}"
                );
            }
            std::fs::remove_dir_all(audit).unwrap();
        }
        ran += 1;
    }
    eprintln!("replayed {ran} baselines");
}

#[test]
fn workshop_baselines_replay_strictly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(root.join("evals/results")).unwrap() {
        let baseline = entry.unwrap().path();
        let name = baseline.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("workshop") {
            continue;
        }
        if !root.join("evals/cache").join(&name).exists() {
            eprintln!("skip {name}: no cache");
            continue;
        }
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
        if doc["workshop_version"] != 3 {
            eprintln!("historical workshop recording {name}: practices removed; re-record");
            continue;
        }
        let out = Command::new(env!("CARGO_BIN_EXE_eval"))
            .arg(
                root.join("evals/scenarios")
                    .join(format!("{}.json", doc["scenario"].as_str().unwrap())),
            )
            .args(["--arm", doc["arm"].as_str().unwrap()])
            .args([
                "--trial",
                &doc["trial"].to_string(),
                "--strict",
                "--baseline",
            ])
            .env("BACKGROUND_DAILY_TOKEN_BUDGET", "100000000")
            .env(
                "MORPHO_DROP_STREAMS",
                doc["drop_streams"].as_str().unwrap_or(""),
            )
            .arg(&baseline)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{name} failed strict replay:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
