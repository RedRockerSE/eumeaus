use eumeaus_domain_lookup_plugin::DomainLookup;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(DomainLookup::new())
        .await
        .expect("domain-lookup plugin server failed");
}
