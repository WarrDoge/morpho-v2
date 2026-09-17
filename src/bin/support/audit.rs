//! Opt-in diagnostics: observes state without adding records or changing prompts.
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf, time::Instant};

use anyhow::Result;
use morpho::{Services, config::settings};
use serde_json::{Value, json};

pub struct Audit {
    pub dir: PathBuf,
    started: Instant,
}

impl Audit {
    pub fn new(dir: PathBuf) -> Result<Self> {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        // Refuse to overwrite evidence from an earlier run.
        fs::create_dir(&dir)?;
        let s = settings();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec_pretty(&json!({
                "model": s.llm_model, "embed_model": s.embed_model, "embed_dim": s.embed_dim,
                "context_token_budget": s.context_token_budget, "max_prompt_tokens": s.max_prompt_tokens,
                "max_completion_tokens": s.max_completion_tokens,
                "background_daily_token_budget": s.background_daily_token_budget,
                "idle_reflect_seconds": s.idle_reflect_seconds,
                "reflect_every_n_events": s.reflect_every_n_events,
                "drop_streams": s.drop_streams,
            }))?,
        )?;
        Ok(Self {
            dir,
            started: Instant::now(),
        })
    }

    pub fn record(&self, svc: &Services, phase: &str, details: Value) -> Result<()> {
        let st = svc.store.lock().unwrap();
        let s = &st.state;
        let state = json!({"working": s.working, "self_state": s.self_state, "narrative": s.narrative,
            "tables": s.tables.iter().map(|(k,t)| (k, &t.rows)).collect::<BTreeMap<_,_>>()});
        // ponytail: full snapshots grow quadratically with turns; stream deltas if artifacts get large.
        let row = json!({"phase": phase, "seconds": self.started.elapsed().as_secs_f64(),
            "details": details, "usage": svc.llm.counts(), "state": state,
            "events": s.events, "proposals": s.proposals, "transitions": s.transitions,
            "cursors": s.cursors});
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("steps.jsonl"))?;
        serde_json::to_writer(&mut file, &row)?;
        writeln!(file)?;
        eprintln!(
            "audit {phase} turn {} ({:.1}s)",
            row["details"]["turn"],
            self.started.elapsed().as_secs_f64()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morpho::{ids::IdGen, llm::Llm, store::Store};

    #[test]
    fn capture_is_observational_and_refuses_existing_output() {
        // Settings are process-wide; the judge test in this binary needs a single vote.
        unsafe { std::env::set_var("JUDGE_VOTES", "1") };
        let dir = std::env::temp_dir().join(IdGen::random().next("morpho-audit"));
        let a = Audit::new(dir.clone()).unwrap();
        let svc = Services::new(
            Store::open(&dir.join("database"), IdGen::random()).unwrap(),
            Llm::Fake(Default::default()),
        );
        let before = svc.store.lock().unwrap().state.clone();
        a.record(&svc, "failure", json!({"error":"test"})).unwrap();
        assert_eq!(svc.store.lock().unwrap().state, before);
        assert_eq!(svc.llm.counts().llm_calls, 0);
        let row: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("steps.jsonl")).unwrap()).unwrap();
        assert_eq!(row["details"]["error"], "test");
        assert!(Audit::new(dir.clone()).is_err());
        drop(svc);
        fs::remove_dir_all(dir).unwrap();
    }
}
