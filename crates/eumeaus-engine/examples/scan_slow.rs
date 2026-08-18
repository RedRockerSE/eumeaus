//! Test fixture only (see scan_ok.rs's doc comment). Same as scan_ok, but
//! sleeps first — used to time worker-pool concurrency: N of these run
//! serially in worker_pool*delay time with pool=1, or ~delay with pool>=N.

use eumeaus_plugin_protocol::{CheckRequest, CheckResult, ConfidenceStatus, EntityFinding};
use eumeaus_plugin_sdk::PluginRuntime;

const DELAY_MS: u64 = 300;

struct ScanSlow;

impl PluginRuntime for ScanSlow {
    fn describe(&self) -> (String, String) {
        ("scan-slow".to_string(), "0.1.0".to_string())
    }

    fn check(&self, request: &CheckRequest) -> Vec<CheckResult> {
        std::thread::sleep(std::time::Duration::from_millis(DELAY_MS));
        vec![CheckResult {
            status: ConfidenceStatus::Found as i32,
            entities: vec![EntityFinding {
                entity_type: "OnlineAccount".to_string(),
                canonical_key: format!("{}-slow-account", request.input_value),
                display_label: String::new(),
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
    eumeaus_plugin_sdk::serve(ScanSlow)
        .await
        .expect("scan-slow plugin server failed");
}
