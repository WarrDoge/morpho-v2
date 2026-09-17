pub mod agents;
pub mod api;
pub mod config;
pub mod context;
pub mod ids;
pub mod interact;
pub mod llm;
pub mod pyfmt;
pub mod snapshots;
pub mod state;
pub mod store;
pub mod telemetry;
pub mod worker;

use llm::Llm;
use store::{Shared, Store};

pub struct Services {
    pub store: Shared,
    pub llm: std::sync::Arc<Llm>,
    pub cycle_lock: tokio::sync::Mutex<()>,
}

impl Services {
    pub fn new(store: Store, llm: Llm) -> Services {
        Services {
            store: store.shared(),
            llm: std::sync::Arc::new(llm),
            cycle_lock: tokio::sync::Mutex::new(()),
        }
    }
}

impl Services {
    pub fn staged(&self) -> Services {
        Services {
            store: self.store.lock().unwrap().fork().shared(),
            llm: self.llm.clone(),
            cycle_lock: tokio::sync::Mutex::new(()),
        }
    }
    pub fn publish(
        &self,
        staged: Services,
        reply: Option<(&str, &serde_json::Value)>,
    ) -> anyhow::Result<()> {
        let st = std::sync::Arc::try_unwrap(staged.store)
            .map_err(|_| anyhow::anyhow!("staged store still in use"))?
            .into_inner()
            .unwrap();
        self.store.lock().unwrap().publish(st, reply)
    }
}
