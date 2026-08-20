use eumeaus_ip_lookup_plugin::IpLookup;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(IpLookup::new())
        .await
        .expect("ip-lookup plugin server failed");
}
