#![allow(dead_code)]
use std::path::PathBuf;

use morpho::Services;
use morpho::ids::IdGen;
use morpho::llm::{Fake, Llm};
use morpho::store::Store;
use serde_json::json;

pub fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("morpho-it-{}-{name}", std::process::id()))
}

pub fn services(name: &str) -> Services {
    let dir = temp_dir(name);
    let _ = std::fs::remove_dir_all(&dir);
    Services::new(
        Store::open(&dir, IdGen::seeded(name)).unwrap(),
        Llm::Fake(Fake::default()),
    )
}

pub fn fake(svc: &Services) -> &Fake {
    match &svc.llm {
        Llm::Fake(f) => f,
        _ => unreachable!(),
    }
}

pub fn event(svc: &Services, text: &str) -> String {
    let row = svc
        .store
        .lock()
        .unwrap()
        .append_event("user_message", "user", json!({"text": text}), None, None)
        .unwrap();
    row["event_id"].as_str().unwrap().to_string()
}
