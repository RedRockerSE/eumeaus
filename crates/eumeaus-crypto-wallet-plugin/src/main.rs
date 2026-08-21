use eumeaus_crypto_wallet_plugin::CryptoWallet;

#[tokio::main]
async fn main() {
    eumeaus_plugin_sdk::serve(CryptoWallet::new())
        .await
        .expect("crypto-wallet plugin server failed");
}
