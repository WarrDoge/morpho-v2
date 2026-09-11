use std::{env, path::PathBuf, str::FromStr, sync::OnceLock};

pub struct Settings {
    pub deepinfra_api_key: String,
    pub llm_base_url: String,
    pub llm_model: String,
    pub embed_model: String,
    pub embed_dim: usize,
    pub context_token_budget: usize,
    pub reflect_every_n_events: i64,
    pub idle_reflect_seconds: f64,
    pub worker_poll_seconds: f64,
    pub worker_inprocess: bool,
    pub memory_half_life_days: f64,
    pub memory_archive_floor: f64,
    pub max_proposals_per_cycle: usize,
    pub dedupe_threshold: f64,
    pub consumer_max_failures: i64,
    pub data_dir: PathBuf,
    pub bind: String,
}

fn var<T: FromStr>(name: &str, default: T) -> T {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn settings() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    S.get_or_init(|| {
        dotenvy::dotenv().ok();
        Settings {
            deepinfra_api_key: var("DEEPINFRA_API_KEY", String::new()),
            llm_base_url: var("LLM_BASE_URL", "https://api.deepinfra.com/v1/openai".into()),
            llm_model: var("LLM_MODEL", "zai-org/GLM-5.3-Flash".into()),
            embed_model: var("EMBED_MODEL", "BAAI/bge-m3".into()),
            embed_dim: var("EMBED_DIM", 1024),
            context_token_budget: var("CONTEXT_TOKEN_BUDGET", 4000),
            reflect_every_n_events: var("REFLECT_EVERY_N_EVENTS", 10),
            idle_reflect_seconds: var("IDLE_REFLECT_SECONDS", 300.0),
            worker_poll_seconds: var("WORKER_POLL_SECONDS", 1.0),
            worker_inprocess: var("WORKER_INPROCESS", true),
            memory_half_life_days: var("MEMORY_HALF_LIFE_DAYS", 30.0),
            memory_archive_floor: var("MEMORY_ARCHIVE_FLOOR", 0.05),
            max_proposals_per_cycle: var("MAX_PROPOSALS_PER_CYCLE", 50),
            dedupe_threshold: var("DEDUPE_THRESHOLD", 0.95),
            consumer_max_failures: var("CONSUMER_MAX_FAILURES", 3),
            data_dir: var("MORPHO_DATA_DIR", PathBuf::from("./data")),
            bind: var("BIND", "127.0.0.1:8000".into()),
        }
    })
}
