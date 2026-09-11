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
pub mod worker;

use llm::Llm;
use store::{Shared, Store};

pub struct Services {
    pub store: Shared,
    pub llm: Llm,
    pub cycle_lock: tokio::sync::Mutex<()>,
}

impl Services {
    pub fn new(store: Store, llm: Llm) -> Services {
        Services {
            store: store.shared(),
            llm,
            cycle_lock: tokio::sync::Mutex::new(()),
        }
    }
}
