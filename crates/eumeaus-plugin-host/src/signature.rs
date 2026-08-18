//! Plugin signature verification (SPEC.md §3.3's `signature` field, §5's
//! "unsigned/invalid refused by default").
//!
//! *Who* the trusted signer is remains SPEC.md §8's open question 2 — this
//! only implements the verification mechanism, parameterized by whatever
//! public key the caller decides to trust.

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::manifest::PluginManifest;
use crate::PluginError;

#[derive(Debug, Clone)]
pub enum TrustPolicy {
    /// Default: refuse to load unless `plugin.signature` verifies against
    /// `trusted_key`.
    RequireSignature { trusted_key: VerifyingKey },
    /// `--allow-unsigned`: load regardless of signature. SPEC.md §5 calls
    /// for "a loud warning" when this is used — that's the CLI's job at
    /// the call site, not this type's.
    AllowUnsigned,
}

/// The exact bytes a plugin author signs: `"{name}\n{version}\n{binary_sha256_hex}"`.
///
/// SPEC.md §3.3 only says "a base64 detached signature over manifest and
/// binary hash" — it doesn't specify an exact encoding. Signing the raw
/// manifest TOML text was considered and rejected: re-serializing it with
/// the `toml` crate to strip the `signature` field before verification
/// isn't guaranteed to byte-for-byte match whatever tool the signer used
/// to produce it, which would make correctly-signed plugins fail to
/// verify. This binds the signature to what actually matters instead —
/// which code runs, under which declared name/version — without that
/// fragility.
fn signable_payload(manifest: &PluginManifest) -> Result<Vec<u8>, PluginError> {
    let binary = std::fs::read(manifest.entrypoint_path())?;
    let binary_hash = Sha256::digest(&binary);
    Ok(format!(
        "{}\n{}\n{binary_hash:x}",
        manifest.plugin.name, manifest.plugin.version
    )
    .into_bytes())
}

pub fn verify(manifest: &PluginManifest, policy: &TrustPolicy) -> Result<(), PluginError> {
    let trusted_key = match policy {
        TrustPolicy::AllowUnsigned => return Ok(()),
        TrustPolicy::RequireSignature { trusted_key } => trusted_key,
    };

    let sig_b64 = manifest
        .plugin
        .signature
        .as_deref()
        .ok_or_else(|| PluginError::Unsigned(manifest.plugin.name.clone()))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig_b64)
        .map_err(|e| PluginError::Signature(format!("invalid base64 signature: {e}")))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| PluginError::Signature("signature must be 64 bytes".to_string()))?;
    let signature = Signature::from_bytes(&sig_bytes);

    let payload = signable_payload(manifest)?;
    trusted_key
        .verify(&payload, &signature)
        .map_err(|_| PluginError::InvalidSignature(manifest.plugin.name.clone()))
}

/// Produces the base64 string that goes in `plugin.toml`'s `signature`
/// field for the given manifest + entrypoint binary. Not just test
/// scaffolding — this is what a future `eumeaus plugin sign` tool would
/// call too.
pub fn sign(
    signing_key: &ed25519_dalek::SigningKey,
    manifest: &PluginManifest,
) -> Result<String, PluginError> {
    use ed25519_dalek::Signer;
    let payload = signable_payload(manifest)?;
    let signature = signing_key.sign(&payload);
    Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()))
}
