mod common;
use common::{event, services};
use morpho::agents::seed::seed_traits;
use morpho::context::composer::{compose_text_as, identity};
use serde_json::json;

#[tokio::test]
async fn dropped_streams_never_reach_the_prompt() {
    unsafe { std::env::set_var("MORPHO_DROP_STREAMS", "recent,self,identity") };
    let svc = services("ablation");
    event(&svc, "Nova launch");
    let (text, manifest) = compose_text_as(&svc, "Nova", "user", Some(4000))
        .await
        .unwrap();
    assert!(!text.contains("Nova launch"));
    assert_eq!(manifest["sections"]["recent"]["included"], 0);
    assert_eq!(manifest["sections"]["recent"]["dropped"], 0);
    assert_eq!(manifest["sections"]["self"]["included"], 0);
    seed_traits(
        &svc,
        &[json!({"kind":"value","statement":"I say what I think."})],
    )
    .await
    .unwrap();
    assert_eq!(identity(&svc.store.lock().unwrap(), None).block, "");
}
