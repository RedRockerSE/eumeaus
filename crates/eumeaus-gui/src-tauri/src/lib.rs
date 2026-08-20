mod case_state;
mod credential_state;
mod entity_state;
mod overview_state;
mod plugin_state;
mod scan_state;
mod trust_state;

use case_state::{case_close, case_create, case_current, case_list, case_open, AppState};
use credential_state::{credential_list, credential_remove, credential_set};
use entity_state::{
    entity_add, entity_list, entity_merge, entity_show, entity_split, relationship_add,
    relationship_list,
};
use overview_state::{audit_list, case_stats};
use plugin_state::{plugin_install, plugin_list, plugin_verify};
use scan_state::{scan_list, scan_run};
use trust_state::{trust_add, trust_list, trust_remove};

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
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    // G6 (SPEC.md §9.4/§9.6): desktop-only, same as every other Tauri
    // project's own setup — this GUI targets Linux/Windows desktop only
    // (§9.7), never mobile, but gating on #[cfg(desktop)] costs nothing
    // and matches upstream's own convention.
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
    }

    builder
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
            relationship_list,
            case_stats,
            audit_list,
            scan_run,
            scan_list,
            plugin_list,
            plugin_install,
            plugin_verify,
            credential_set,
            credential_list,
            credential_remove,
            trust_add,
            trust_list,
            trust_remove,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
