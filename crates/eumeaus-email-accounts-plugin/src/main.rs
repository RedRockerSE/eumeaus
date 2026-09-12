use eumeaus_email_accounts_plugin::EmailAccounts;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(EmailAccounts::new())
        .await
        .expect("email-accounts plugin server failed");
}
