//! Test fixture only (see stub_echo.rs's doc comment). Handshakes
//! normally, then blocks the calling thread forever on `Check` — used by
//! tests/host.rs as "a deliberately-hanging stub plugin" (SPEC.md §7, M3
//! verify criterion). `PluginHandle`'s `kill_on_drop` guarantees this
//! never outlives the test regardless of the sleep duration.

use eumeaus_plugin_protocol::{CheckRequest, CheckResult};
use eumeaus_plugin_sdk::PluginRuntime;

struct Hang;

#[async_trait::async_trait]
impl PluginRuntime for Hang {
    fn describe(&self) -> (String, String) {
        ("stub-hang".to_string(), "0.1.0".to_string())
    }

    async fn check(&self, _request: &CheckRequest) -> Vec<CheckResult> {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        vec![]
    }
}

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(Hang)
        .await
        .expect("stub-hang plugin server failed");
}
