use std::sync::Arc;

use anyhow::Result;
use tracing::level_filters::LevelFilter;

use morpho::config::settings;
use morpho::ids::IdGen;
use morpho::llm::{DeepInfra, Llm};
use morpho::store::Store;
use morpho::worker::run_forever;
use morpho::{Services, api};

#[tokio::main]
async fn main() -> Result<()> {
    let s = settings();
    morpho::telemetry::init("morpho", LevelFilter::INFO, None)?;
    let store = Store::open(&s.data_dir, IdGen::random())?;
    tracing::info!(
        "opened {} ({} events, {} transitions)",
        s.data_dir.display(),
        store.state.events.len(),
        store.state.transitions.len()
    );
    let svc = Arc::new(Services::new(store, Llm::Live(DeepInfra::new())));
    if !s.seed_file.is_empty() {
        let traits: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&s.seed_file)?)?;
        let n = morpho::agents::seed::seed_traits(&svc, &traits).await?;
        tracing::info!("seeded {n} traits");
    }
    {
        let worker = svc.clone();
        tokio::spawn(async move { run_forever(&worker).await });
    }
    let listener = tokio::net::TcpListener::bind(&s.bind).await?;
    tracing::info!("listening on {}", s.bind);
    axum::serve(listener, api::router(svc)).await?;
    Ok(())
}
