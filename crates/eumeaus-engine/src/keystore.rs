//! Encryption-key storage in the OS-native credential store (Keychain /
//! Credential Manager / Secret Service), referenced by the case's UUID.
//! Per SPEC.md §4.1/§4.5, the key never touches the case file itself.

use uuid::Uuid;

use crate::EngineError;

const SERVICE: &str = "eumeaus";
const KEY_BYTES: usize = 32; // AES-256, per SQLCipher's default cipher.

fn entry(case_id: Uuid) -> Result<keyring::Entry, EngineError> {
    keyring::Entry::new(SERVICE, &case_id.to_string())
        .map_err(|e| EngineError::Keychain(e.to_string()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Generates a fresh random key and stores it under `case_id`. Returns the
/// key as a lowercase hex string, ready for a SQLCipher raw-key PRAGMA
/// (`PRAGMA key = "x'<hex>'"`).
pub(crate) fn create_key(case_id: Uuid) -> Result<String, EngineError> {
    let mut bytes = [0u8; KEY_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| EngineError::Keychain(e.to_string()))?;
    let hex_key = to_hex(&bytes);
    entry(case_id)?
        .set_password(&hex_key)
        .map_err(|e| EngineError::Keychain(e.to_string()))?;
    Ok(hex_key)
}

/// Looks up the key previously stored by [`create_key`] for `case_id`.
pub(crate) fn load_key(case_id: Uuid) -> Result<String, EngineError> {
    entry(case_id)?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => EngineError::KeyNotFound(case_id),
        other => EngineError::Keychain(other.to_string()),
    })
}

/// Best-effort cleanup used when case creation fails partway through, so a
/// retry doesn't leave an orphaned keychain entry behind. Deliberately
/// swallows lookup errors from [`entry`] itself.
pub(crate) fn delete_key(case_id: Uuid) -> Result<(), EngineError> {
    match entry(case_id) {
        Ok(entry) => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(EngineError::Keychain(e.to_string())),
        },
        Err(e) => Err(e),
    }
}
