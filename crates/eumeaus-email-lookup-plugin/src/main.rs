use eumeaus_email_lookup_plugin::EmailLookup;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(EmailLookup::new())
        .await
        .expect("email-lookup plugin server failed");
}
