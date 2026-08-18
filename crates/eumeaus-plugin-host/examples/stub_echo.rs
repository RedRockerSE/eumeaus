//! Test fixture only (built via a dev-dependency on eumeaus-plugin-sdk —
//! see Cargo.toml). Responds immediately with one FOUND result echoing the
//! request. Used by tests/host.rs as "a trivial stub plugin" (SPEC.md §7,
//! M3 verify criterion).

use eumeaus_plugin_protocol::{CheckRequest, CheckResult, ConfidenceStatus, EntityFinding};
use eumeaus_plugin_sdk::PluginRuntime;

struct Echo;

#[async_trait::async_trait]
impl PluginRuntime for Echo {
    fn describe(&self) -> (String, String) {
        ("stub-echo".to_string(), "0.1.0".to_string())
    }

    async fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        vec![CheckResult {
            status: ConfidenceStatus::Found as i32,
            entities: vec![EntityFinding {
                entity_type: request.input_entity_type.clone(),
                canonical_key: request.input_value.clone(),
                display_label: request.input_value.clone(),
                attributes: Default::default(),
            }],
            relationships: vec![],
            provenance: None,
            error_message: String::new(),
        }]
    }
}

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(Echo)
        .await
        .expect("stub-echo plugin server failed");
}
