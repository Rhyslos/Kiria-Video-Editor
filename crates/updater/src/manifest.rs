//! The signed list of files in a release.
//!
//! Every release carries `manifest.json` (version, channel, each file's size
//! and SHA-256) and `manifest.json.sig` (an ed25519 signature of the exact
//! manifest bytes, base64). The app trusts a download only when the
//! signature matches the public key built into it *and* the file's
//! checksum matches the manifest.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use diag::{AppError, Explain};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The release signing key's public half. Its private half exists only as
/// the `RELEASE_SIGNING_KEY` secret of the GitHub repository.
pub const RELEASE_PUBLIC_KEY: [u8; 32] = [
    0x29, 0x45, 0xe9, 0x8d, 0x2e, 0xee, 0xf6, 0x3a, 0x72, 0x79, 0xe0, 0x2b, 0x3d, 0x2d, 0x03, 0x4c, 0x1e,
    0x6c, 0x5a, 0xb2, 0x38, 0x87, 0x1b, 0x63, 0x13, 0x31, 0xc6, 0x0b, 0x12, 0x8d, 0xa4, 0x68,
];

/// Stable releases, or every release including betas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The release version, for example "0.3.1".
    pub version: String,
    pub channel: Channel,
    /// The oldest launcher that can install this release (Phase 3).
    pub min_launcher_version: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

impl Manifest {
    /// The manifest bytes as published (and signed).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }

    /// Checks the signature, then reads the manifest. Nothing in an unsigned
    /// or altered manifest is ever looked at.
    pub fn verify_and_parse(
        bytes: &[u8],
        signature_base64: &str,
        public_key: &[u8; 32],
    ) -> diag::Result<Manifest> {
        let key = VerifyingKey::from_bytes(public_key).or_explain(tampered)?;
        let sig_bytes = BASE64.decode(signature_base64.trim()).or_explain(tampered)?;
        let signature = Signature::from_slice(&sig_bytes).or_explain(tampered)?;
        key.verify(bytes, &signature).or_explain(tampered)?;
        serde_json::from_slice(bytes).or_explain(tampered)
    }

    /// The entry for a file name.
    pub fn file(&self, name: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|f| f.name == name)
    }
}

/// Signs manifest bytes; used by the release workflow (`cargo xtask sign`).
pub fn sign(bytes: &[u8], secret_key: &[u8; 32]) -> String {
    let signature = SigningKey::from_bytes(secret_key).sign(bytes);
    BASE64.encode(signature.to_bytes())
}

/// The public key that belongs to a secret key.
pub fn public_key_of(secret_key: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(secret_key).verifying_key().to_bytes()
}

/// SHA-256 of some bytes, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn tampered() -> AppError {
    AppError::new("The update was refused")
        .why(
            "Its signature or checksum does not match: it was damaged in transit or changed by someone else.",
        )
        .fix("Nothing was installed. Try again later; if it keeps happening, report it.")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: [u8; 32] = [7; 32];

    fn manifest() -> Manifest {
        Manifest {
            version: "0.1.0".into(),
            channel: Channel::Stable,
            min_launcher_version: "0.1.0".into(),
            files: vec![ManifestFile {
                name: "app-windows-x64.zip".into(),
                size: 3,
                sha256: sha256_hex(b"abc"),
            }],
        }
    }

    #[test]
    fn a_signed_manifest_verifies() {
        let bytes = manifest().to_bytes();
        let sig = sign(&bytes, &TEST_SECRET);
        let parsed = Manifest::verify_and_parse(&bytes, &sig, &public_key_of(&TEST_SECRET)).unwrap();
        assert_eq!(parsed, manifest());
    }

    #[test]
    fn a_changed_manifest_is_refused() {
        let bytes = manifest().to_bytes();
        let sig = sign(&bytes, &TEST_SECRET);
        let mut altered = bytes.clone();
        let i = altered.iter().position(|b| *b == b'1').unwrap();
        altered[i] = b'9';
        let err = Manifest::verify_and_parse(&altered, &sig, &public_key_of(&TEST_SECRET)).unwrap_err();
        assert!(err.what().contains("refused"));
    }

    #[test]
    fn a_manifest_signed_with_another_key_is_refused() {
        let bytes = manifest().to_bytes();
        let sig = sign(&bytes, &[9; 32]);
        assert!(Manifest::verify_and_parse(&bytes, &sig, &public_key_of(&TEST_SECRET)).is_err());
        assert!(Manifest::verify_and_parse(&bytes, "not base64!", &public_key_of(&TEST_SECRET)).is_err());
    }

    #[test]
    fn sha256_matches_a_known_value() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
