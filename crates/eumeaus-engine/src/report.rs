//! Report signing/verification (SPEC.md §8 open question 6): a generic
//! Ed25519 detached signature over any export file — most useful for
//! `ExportFormat::Report`/`ExportFormat::Html`, which (unlike the
//! SQLCipher export formats) have no tamper-evidence of their own.
//!
//! The investigator brings their own signing key; nothing here generates
//! or custodies one, same philosophy as the trust store (SPEC.md §8.2):
//! `sign_export` takes a hex-encoded private key (e.g. read from
//! `--sign-key-file`, generated with whatever standard Ed25519 tool the
//! investigator already trusts), and `verify_report` checks a signature
//! against a public key the caller already resolved (from `--trusted-key`
//! or the local `trust` store — see `crate::trust`).

use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;

use crate::EngineError;

/// Signs `dest`'s current contents, writing a detached signature to
/// `<dest>.sig`. Returns the sig path and the signer's public key (hex),
/// so a caller can print it for the investigator to `trust add` or hand
/// to a report recipient directly.
pub fn sign_export(dest: &Path, signing_key_hex: &str) -> Result<(PathBuf, String), EngineError> {
    let signing_key =
        eumeaus_plugin_host::detached_signature::signing_key_from_hex(signing_key_hex.trim())?;
    let data = std::fs::read(dest)?;
    let signature = eumeaus_plugin_host::detached_signature::sign(&signing_key, &data);

    let mut sig_path = dest.to_path_buf();
    let mut file_name = dest
        .file_name()
        .expect("export dest has a filename")
        .to_os_string();
    file_name.push(".sig");
    sig_path.set_file_name(file_name);
    std::fs::write(&sig_path, &signature)?;

    let public_key_hex = signing_key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((sig_path, public_key_hex))
}

/// Verifies a detached signature (as written by [`sign_export`]) over
/// `report_path` against `trusted_key`.
pub fn verify_report(
    report_path: &Path,
    sig_path: &Path,
    trusted_key: VerifyingKey,
) -> Result<(), EngineError> {
    let data = std::fs::read(report_path)?;
    let signature = std::fs::read_to_string(sig_path)?;
    Ok(eumeaus_plugin_host::detached_signature::verify(
        &trusted_key,
        &data,
        signature.trim(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        std::fs::write(&report_path, b"{\"entities\":[]}").unwrap();

        let signing_key = SigningKey::generate(&mut OsRng);
        let signing_key_hex = hex_of(&signing_key.to_bytes());

        let (sig_path, public_key_hex) = sign_export(&report_path, &signing_key_hex).unwrap();
        assert!(sig_path.exists());
        assert_eq!(
            public_key_hex,
            hex_of(&signing_key.verifying_key().to_bytes())
        );

        verify_report(&report_path, &sig_path, signing_key.verifying_key()).unwrap();
    }

    #[test]
    fn verify_rejects_a_report_tampered_after_signing() {
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        std::fs::write(&report_path, b"{\"entities\":[]}").unwrap();

        let signing_key = SigningKey::generate(&mut OsRng);
        let (sig_path, _) = sign_export(&report_path, &hex_of(&signing_key.to_bytes())).unwrap();

        std::fs::write(&report_path, b"{\"entities\":[\"forged\"]}").unwrap();

        let err = verify_report(&report_path, &sig_path, signing_key.verifying_key()).unwrap_err();
        assert!(matches!(err, EngineError::PluginHost(_)));
    }

    #[test]
    fn verify_rejects_the_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("report.json");
        std::fs::write(&report_path, b"{\"entities\":[]}").unwrap();

        let signing_key = SigningKey::generate(&mut OsRng);
        let (sig_path, _) = sign_export(&report_path, &hex_of(&signing_key.to_bytes())).unwrap();

        let wrong_key = SigningKey::generate(&mut OsRng);
        let err = verify_report(&report_path, &sig_path, wrong_key.verifying_key()).unwrap_err();
        assert!(matches!(err, EngineError::PluginHost(_)));
    }
}
