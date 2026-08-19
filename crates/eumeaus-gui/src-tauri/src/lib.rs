// G0 (SPEC.md §9.6): a trivial command that genuinely round-trips into
// eumeaus-engine, proving the workspace dependency links correctly — not
// just a string literal echoed back. Case-backed commands start at G1.
#[tauri::command]
fn list_entity_types() -> Vec<String> {
    use eumeaus_engine::EntityType::*;
    [
        Person,
        Username,
        Email,
        PhoneNumber,
        Domain,
        IpAddress,
        OnlineAccount,
        Organization,
        Location,
        Document,
        Image,
        Vehicle,
        CryptoWallet,
        Url,
    ]
    .into_iter()
    .map(|t| t.to_string())
    .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![list_entity_types])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
