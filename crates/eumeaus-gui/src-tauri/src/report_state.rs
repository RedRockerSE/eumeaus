//! Report export + signature verify (SPEC.md §9.3) — the one screen from
//! the original §9.3 list that neither G0–G6 nor the Claude Design
//! handover ended up building; added after the dogfooding pass surfaced
//! it as a real gap. Wraps `Case::export` (all four `ExportFormat`
//! variants) and `report::sign_export`/`verify_report`, the same calls
//! `eumeaus-cli`'s `case export`/`report verify` make.
//!
//! `case_export` is case-scoped (needs `case_state::AppState`);
//! `report_verify` isn't — a report and its `.sig` are standalone files,
//! same as `eumeaus-cli`'s own `report verify` (no `--case`).

use std::path::Path;
use std::sync::{Arc, Mutex};

use ed25519_dalek::VerifyingKey;
use eumeaus_engine::{report, Case, ExportFormat};

const NO_CASE_OPEN: &str = "no case is currently open — open a case first";

fn hex_to_verifying_key(hex: &str) -> Result<VerifyingKey, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("odd-length hex string".to_string());
    }
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "trusted key must be 32 bytes (64 hex chars)".to_string())?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| format!("invalid trusted key: {e}"))
}

fn resolve_verifying_key(
    trusted_key: Option<&str>,
    trust: Option<&str>,
) -> Result<VerifyingKey, String> {
    match (trusted_key, trust) {
        (Some(hex), None) => hex_to_verifying_key(hex),
        (None, Some(name)) => eumeaus_engine::trust::resolve(name).map_err(|e| e.to_string()),
        (None, None) => {
            Err("pass a trusted key or a trust-store name to verify against".to_string())
        }
        (Some(_), Some(_)) => Err("pass only one of trusted_key or trust, not both".to_string()),
    }
}

fn do_case_export(
    cell: &Arc<Mutex<Option<Case>>>,
    out: &Path,
    format: &str,
    passphrase: Option<&str>,
    sign_key_hex: Option<&str>,
) -> Result<Option<String>, String> {
    let guard = cell.lock().unwrap();
    let case = guard.as_ref().ok_or(NO_CASE_OPEN)?;

    let export_format = match format {
        "sqlite" => ExportFormat::Sqlite,
        "report" => ExportFormat::Report,
        "html" => ExportFormat::Html,
        "portable" => {
            let p = passphrase.ok_or("a passphrase is required for a portable export")?;
            ExportFormat::Portable(p.to_string())
        }
        other => return Err(format!("unknown export format {other:?}")),
    };
    case.export(out, export_format).map_err(|e| e.to_string())?;

    match sign_key_hex {
        Some(key_hex) if !key_hex.is_empty() => {
            let (_sig_path, public_key_hex) =
                report::sign_export(out, key_hex).map_err(|e| e.to_string())?;
            Ok(Some(public_key_hex))
        }
        _ => Ok(None),
    }
}

fn do_report_verify(
    report_path: &Path,
    sig_path: &Path,
    trusted_key: Option<&str>,
    trust: Option<&str>,
) -> Result<(), String> {
    let key = resolve_verifying_key(trusted_key, trust)?;
    report::verify_report(report_path, sig_path, key).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn case_export(
    state: tauri::State<'_, crate::case_state::AppState>,
    out: String,
    format: String,
    passphrase: Option<String>,
    sign_key_hex: Option<String>,
) -> Result<Option<String>, String> {
    let cell = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        do_case_export(
            &cell,
            Path::new(&out),
            &format,
            passphrase.as_deref(),
            sign_key_hex.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn report_verify(
    report_path: String,
    sig_path: String,
    trusted_key: Option<String>,
    trust: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        do_report_verify(
            Path::new(&report_path),
            Path::new(&sig_path),
            trusted_key.as_deref(),
            trust.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use eumeaus_engine::EntityType;

    fn tmp_cell_with_case(case: Case) -> Arc<Mutex<Option<Case>>> {
        Arc::new(Mutex::new(Some(case)))
    }

    #[test]
    fn export_and_verify_error_cleanly_with_no_case_open_or_bad_trust() {
        let cell: Arc<Mutex<Option<Case>>> = Arc::new(Mutex::new(None));
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.json");
        assert_eq!(
            do_case_export(&cell, &out, "report", None, None).unwrap_err(),
            NO_CASE_OPEN
        );

        let err = do_report_verify(&out, &out, None, None).unwrap_err();
        assert!(err.contains("trust"));
    }

    #[test]
    fn portable_export_without_a_passphrase_errors() {
        let dir = tempfile::tempdir().unwrap();
        let case = Case::create(dir.path(), "g-export").unwrap();
        let cell = tmp_cell_with_case(case);
        let out = dir.path().join("portable.eum");

        let err = do_case_export(&cell, &out, "portable", None, None).unwrap_err();
        assert!(err.contains("passphrase"));
    }

    // The real proof: a JSON report exported through this exact code path
    // signs and verifies correctly against the real detached-signature
    // scheme eumeaus-cli's own `case export --sign-key-file`/`report
    // verify` use (SPEC.md §8 open question 6).
    #[test]
    fn report_export_sign_and_verify_round_trips() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;

        let dir = tempfile::tempdir().unwrap();
        let mut case = Case::create(dir.path(), "g-export-sign").unwrap();
        case.add_entity(
            EntityType::Person,
            None,
            vec![],
            eumeaus_engine::Provenance {
                source: "user".to_string(),
                source_version: "0.1.0".to_string(),
                source_url: None,
                retrieval_method: None,
                raw_response_sha256: None,
                collected_at_unix_ms: 0,
            },
        )
        .unwrap();
        let cell = tmp_cell_with_case(case);
        let out = dir.path().join("report.json");

        let signing_key = SigningKey::generate(&mut OsRng);
        let signing_key_hex: String = signing_key
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let public_key_hex = do_case_export(&cell, &out, "report", None, Some(&signing_key_hex))
            .unwrap()
            .expect("signing was requested, so a public key should come back");

        let sig_path = dir.path().join("report.json.sig");
        assert!(sig_path.exists());

        do_report_verify(&out, &sig_path, Some(&public_key_hex), None).unwrap();

        // Tampering after signing must fail verification.
        std::fs::write(&out, b"{\"tampered\": true}").unwrap();
        assert!(do_report_verify(&out, &sig_path, Some(&public_key_hex), None).is_err());
    }
}
