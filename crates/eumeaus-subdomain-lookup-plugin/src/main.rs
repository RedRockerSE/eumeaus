use eumeaus_subdomain_lookup_plugin::SubdomainLookup;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(SubdomainLookup::new())
        .await
        .expect("subdomain-lookup plugin server failed");
}
