use std::{env, path::PathBuf, str::FromStr, sync::OnceLock};

pub struct Settings {
    pub deepinfra_api_key: String,
    pub llm_base_url: String,
    pub llm_model: String,
    pub llm_timeout_seconds: u64,
    pub embed_model: String,
    pub embed_dim: usize,
    pub context_token_budget: usize,
    pub context_score_floor: f64,
    pub max_prompt_tokens: u64,
    pub max_completion_tokens: u64,
    pub background_daily_token_budget: u64,
    pub reflect_every_n_events: i64,
    pub idle_reflect_seconds: f64,
    pub worker_poll_seconds: f64,
    pub worker_inprocess: bool,
    pub memory_half_life_days: f64,
    pub memory_archive_floor: f64,
    pub decay_keeps_cited_evidence: bool,
    pub max_proposals_per_cycle: usize,
    pub consumer_max_failures: i64,
    pub drop_streams: String,
    pub speaker_share: f64,
    pub premise_chars: usize,
    pub reflect_surprisal_top: f64,
    pub seed_file: String,
    pub identity_bias: f64,
    pub judge_model: String,
    pub judge_votes: usize,
    pub judge_temperature: Option<f64>,
    pub clerk_model: String,
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
            llm_timeout_seconds: var("LLM_TIMEOUT_SECONDS", 120),
            embed_model: var("EMBED_MODEL", "BAAI/bge-m3".into()),
            embed_dim: var("EMBED_DIM", 1024),
            max_prompt_tokens: var("MAX_PROMPT_TOKENS", 12000),
            max_completion_tokens: var("MAX_COMPLETION_TOKENS", 2048),
            background_daily_token_budget: var("BACKGROUND_DAILY_TOKEN_BUDGET", 100000),
            context_token_budget: var("CONTEXT_TOKEN_BUDGET", 4000),
            context_score_floor: var("CONTEXT_SCORE_FLOOR", 0.0),
            reflect_every_n_events: var("REFLECT_EVERY_N_EVENTS", 10),
            idle_reflect_seconds: var("IDLE_REFLECT_SECONDS", 300.0),
            worker_poll_seconds: var("WORKER_POLL_SECONDS", 1.0),
            worker_inprocess: var("WORKER_INPROCESS", true),
            memory_half_life_days: var("MEMORY_HALF_LIFE_DAYS", 30.0),
            memory_archive_floor: var("MEMORY_ARCHIVE_FLOOR", 0.05),
            decay_keeps_cited_evidence: var("DECAY_KEEPS_CITED_EVIDENCE", false),
            max_proposals_per_cycle: var("MAX_PROPOSALS_PER_CYCLE", 50),
            consumer_max_failures: var("CONSUMER_MAX_FAILURES", 3),
            drop_streams: var("MORPHO_DROP_STREAMS", String::new()),
            speaker_share: var("SPEAKER_SHARE", 0.0),
            premise_chars: var("PREMISE_CHARS", 0),
            reflect_surprisal_top: var("REFLECT_SURPRISAL_TOP", 0.0),
            seed_file: var("MORPHO_SEED_FILE", String::new()),
            identity_bias: var("IDENTITY_BIAS", 0.2),
            judge_model: var("JUDGE_MODEL", String::new()),
            judge_votes: var("JUDGE_VOTES", 3),
            judge_temperature: env::var("JUDGE_TEMPERATURE")
                .ok()
                .and_then(|v| v.parse().ok()),
            clerk_model: var("CLERK_MODEL", String::new()),
            data_dir: var("MORPHO_DATA_DIR", PathBuf::from("./data")),
            bind: var("BIND", "127.0.0.1:8000".into()),
        }
    })
}
