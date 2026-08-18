use eumeaus_username_search_plugin::UsernameSearch;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(UsernameSearch::new())
        .await
        .expect("username-search plugin server failed");
}
