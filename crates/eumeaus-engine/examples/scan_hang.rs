//! Test fixture only (see scan_ok.rs's doc comment). Handshakes normally,
//! then blocks forever on Check — used to prove one hung plugin in a scan
//! times out (scan_plugin_runs -> TIMEOUT) without blocking the other
//! plugins in the same scan or the scan overall.

use eumeaus_plugin_protocol::{CheckRequest, CheckResult};
use eumeaus_plugin_sdk::PluginRuntime;

struct ScanHang;

impl PluginRuntime for ScanHang {
    fn describe(&self) -> (String, String) {
        ("scan-hang".to_string(), "0.1.0".to_string())
    }

    fn check(&self, _request: &CheckRequest) -> Vec<CheckResult> {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        vec![]
    }
}

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(ScanHang)
        .await
        .expect("scan-hang plugin server failed");
}
