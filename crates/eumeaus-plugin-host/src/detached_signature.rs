//! Generic Ed25519 sign/verify over arbitrary bytes, base64-encoded — the
//! same primitive `signature.rs` uses for plugin manifests, but here
//! parameterized over any content instead of a fixed
//! name/version/binary-hash payload. Used by `case export --sign-key-file`/
//! `eumeaus report verify` (SPEC.md §8 open question 6, evidentiary report
//! format) to make a plaintext report (unlike the SQLCipher export
//! formats, which are tamper-evident on their own via SQLCipher's per-page
//! HMAC) tamper-evident too.

use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::PluginError;

/// Signs `data`, returning a base64-encoded detached signature.
pub fn sign(signing_key: &SigningKey, data: &[u8]) -> String {
    let signature = signing_key.sign(data);
    base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
}

/// Verifies a base64-encoded detached signature (as produced by [`sign`])
/// over `data` against `verifying_key`.
pub fn verify(
    verifying_key: &VerifyingKey,
    data: &[u8],
    signature_b64: &str,
) -> Result<(), PluginError> {
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .map_err(|e| PluginError::Signature(format!("invalid base64 signature: {e}")))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| PluginError::Signature("signature must be 64 bytes".to_string()))?;
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(data, &signature)
        .map_err(|_| PluginError::Signature("signature does not verify".to_string()))
}

/// Parses a hex-encoded 32-byte Ed25519 private key seed (e.g. read from
/// `--sign-key-file`). The investigator generates and manages this key
/// entirely outside this tool with whatever they already trust (a
/// standard Ed25519 keygen tool) — same "bring your own key" philosophy
/// as the trust store (SPEC.md §8.2): nothing here generates or
/// custodies an investigator's identity key.
pub fn signing_key_from_hex(hex: &str) -> Result<SigningKey, PluginError> {
    if hex.len() != 64 {
        return Err(PluginError::Signature(
            "signing key must be exactly 64 hex characters (32 bytes)".to_string(),
        ));
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| PluginError::Signature(format!("invalid hex: {e}")))?;
    }
    Ok(SigningKey::from_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn sign_then_verify_round_trips() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let data = b"the contents of a report file";

        let sig = sign(&signing_key, data);
        verify(&signing_key.verifying_key(), data, &sig).unwrap();
    }

    #[test]
    fn verify_rejects_the_wrong_key() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng);
        let data = b"the contents of a report file";

        let sig = sign(&signing_key, data);
        let err = verify(&wrong_key.verifying_key(), data, &sig).unwrap_err();
        assert!(matches!(err, PluginError::Signature(_)));
    }

    #[test]
    fn verify_rejects_tampered_data() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let sig = sign(&signing_key, b"original contents");

        let err = verify(&signing_key.verifying_key(), b"tampered contents", &sig).unwrap_err();
        assert!(matches!(err, PluginError::Signature(_)));
    }

    #[test]
    fn signing_key_from_hex_round_trips() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let hex: String = signing_key
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let parsed = signing_key_from_hex(&hex).unwrap();
        assert_eq!(parsed.to_bytes(), signing_key.to_bytes());
    }

    #[test]
    fn signing_key_from_hex_rejects_wrong_length() {
        let err = signing_key_from_hex("not64chars").unwrap_err();
        assert!(matches!(err, PluginError::Signature(_)));
    }
}
