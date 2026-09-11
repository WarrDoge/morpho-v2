use std::sync::Arc;

use anyhow::Result;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

use morpho::config::settings;
use morpho::ids::IdGen;
use morpho::llm::{DeepInfra, Llm};
use morpho::store::Store;
use morpho::worker::run_forever;
use morpho::{Services, api};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();
    let s = settings();
    let store = Store::open(&s.data_dir, IdGen::random())?;
    tracing::info!(
        "opened {} ({} events, {} transitions)",
        s.data_dir.display(),
        store.state.events.len(),
        store.state.transitions.len()
    );
    let svc = Arc::new(Services::new(store, Llm::Live(DeepInfra::new())));
    {
        let worker = svc.clone();
        tokio::spawn(async move { run_forever(&worker).await });
    }
    let listener = tokio::net::TcpListener::bind(&s.bind).await?;
    tracing::info!("listening on {}", s.bind);
    axum::serve(listener, api::router(svc)).await?;
    Ok(())
}
