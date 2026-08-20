mod case_state;
mod entity_state;
mod scan_state;

use case_state::{case_close, case_create, case_current, case_list, case_open, AppState};
use entity_state::{
    entity_add, entity_list, entity_merge, entity_show, entity_split, relationship_add,
};
use scan_state::{scan_list, scan_run};

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
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_entity_types,
            case_create,
            case_open,
            case_close,
            case_current,
            case_list,
            entity_list,
            entity_show,
            entity_add,
            entity_merge,
            entity_split,
            relationship_add,
            scan_run,
            scan_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
