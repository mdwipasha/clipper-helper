//! Component manifest schema, verification, and anti-rollback engine (ARCHITECTURE §9, §13; SECURITY §7–§10).

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ManifestError {
    #[error("Malformed signature JSON: {0}")]
    MalformedSignature(String),
    #[error("Unknown key ID: {0}")]
    UnknownKeyId(String),
    #[error("Invalid Ed25519 signature: {0}")]
    BadSignature(String),
    #[error("Malformed manifest JSON: {0}")]
    MalformedManifest(String),
    #[error("Unsupported manifest schema version: {0}")]
    UnsupportedSchema(u32),
    #[error(
        "Manifest sequence rollback rejected: received {received}, expected >= {last_accepted}"
    )]
    SequenceRollback { received: u64, last_accepted: u64 },
    #[error("Public key decoding error: {0}")]
    KeyDecodeError(String),
}

impl ManifestError {
    pub fn error_code(&self) -> &'static str {
        "E_VERIFY_FAILED"
    }
}

/// Embedded public key entry with kid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestPublicKey {
    pub kid: &'static str,
    pub public_key_hex: &'static str,
}

/// Pinned embedded manifest signing keys (active + next).
/// Kept strictly separate from ticket signing keys per SECURITY §8.
pub const EMBEDDED_MANIFEST_KEYS: &[ManifestPublicKey] = &[
    ManifestPublicKey {
        kid: "clipper-manifest-2026-active",
        public_key_hex: "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29",
    },
    ManifestPublicKey {
        kid: "clipper-manifest-2026-next",
        public_key_hex: "a51a5c657a79e6005c28f2e21b06f851082c5d1e8ef0ce7e3e46c7ad7f94e9f7",
    },
];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ManifestFileEntry {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ComponentManifestEntry {
    pub component: String,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub archive: String, // "none" | "zip"
    #[serde(default)]
    pub files: Vec<ManifestFileEntry>,
    pub min_app_version: String,
    #[serde(default)]
    pub max_app_version: Option<String>,
    #[serde(default = "default_rollout_percent")]
    pub rollout_percent: u8,
}

fn default_rollout_percent() -> u8 {
    100
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ComponentManifest {
    pub schema: u32,
    pub sequence: u64,
    pub generated_at: String,
    pub channel: String,
    pub components: Vec<ComponentManifestEntry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ManifestSignature {
    pub kid: String,
    pub signature: String,
}

/// Parses semantic version `(major, minor, patch)`.
pub fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let s = v.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .and_then(|p| p.split('-').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor, patch))
}

/// Checks if the current app version satisfies `min_app_version <= app <= max_app_version`.
pub fn is_app_version_compatible(
    app_version: &str,
    min_version: &str,
    max_version: Option<&str>,
) -> bool {
    let app = match parse_semver(app_version) {
        Some(v) => v,
        None => return false,
    };
    if let Some(min) = parse_semver(min_version) {
        if app < min {
            return false;
        }
    }
    if let Some(max_str) = max_version {
        if let Some(max) = parse_semver(max_str) {
            if app > max {
                return false;
            }
        }
    }
    true
}

/// Evaluates whether the current installation is included in a staged rollout.
/// Computes a stable hash from `install_id + component + version` modulo 100.
pub fn is_in_rollout(
    install_id: &str,
    component: &str,
    version: &str,
    rollout_percent: u8,
) -> bool {
    if rollout_percent >= 100 {
        return true;
    }
    if rollout_percent == 0 {
        return false;
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    install_id.hash(&mut hasher);
    component.hash(&mut hasher);
    version.hash(&mut hasher);
    let bucket = (hasher.finish() % 100) as u8;
    bucket < rollout_percent
}

/// Verifies a detached Ed25519 signature over exact manifest bytes, verifies sequence monotonicity,
/// and returns the decoded and filtered `ComponentManifest`.
pub fn verify_manifest(
    raw_manifest_bytes: &[u8],
    raw_sig_bytes: &[u8],
    app_version: &str,
    install_id: &str,
    last_accepted_seq: u64,
    extra_keys: Option<&[ManifestPublicKey]>,
) -> Result<ComponentManifest, ManifestError> {
    // 1. Parse signature JSON
    let sig_payload: ManifestSignature = serde_json::from_slice(raw_sig_bytes)
        .map_err(|e| ManifestError::MalformedSignature(e.to_string()))?;

    // 2. Locate public key by kid
    let mut matching_hex = None;
    for pk in EMBEDDED_MANIFEST_KEYS {
        if pk.kid == sig_payload.kid {
            matching_hex = Some(pk.public_key_hex);
            break;
        }
    }
    if matching_hex.is_none() {
        if let Some(extras) = extra_keys {
            for pk in extras {
                if pk.kid == sig_payload.kid {
                    matching_hex = Some(pk.public_key_hex);
                    break;
                }
            }
        }
    }
    let pub_key_hex =
        matching_hex.ok_or_else(|| ManifestError::UnknownKeyId(sig_payload.kid.clone()))?;

    // 3. Decode public key and signature bytes
    let pub_key_bytes = hex::decode(pub_key_hex)
        .map_err(|e| ManifestError::KeyDecodeError(format!("Invalid public key hex: {}", e)))?;
    let pub_key_arr: [u8; 32] = pub_key_bytes
        .try_into()
        .map_err(|_| ManifestError::KeyDecodeError("Public key must be 32 bytes".to_string()))?;
    let verifying_key = VerifyingKey::from_bytes(&pub_key_arr)
        .map_err(|e| ManifestError::KeyDecodeError(e.to_string()))?;

    let sig_bytes = if let Ok(h) = hex::decode(&sig_payload.signature) {
        h
    } else {
        // Fall back to base64 or reject
        return Err(ManifestError::BadSignature(
            "Invalid signature encoding (expected hex)".into(),
        ));
    };
    let sig_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| ManifestError::BadSignature("Signature must be 64 bytes".to_string()))?;
    let signature = Signature::from_bytes(&sig_arr);

    // 4. Verify signature over EXACT served manifest bytes
    verifying_key
        .verify_strict(raw_manifest_bytes, &signature)
        .map_err(|e| ManifestError::BadSignature(e.to_string()))?;

    // 5. Parse manifest JSON
    let manifest: ComponentManifest = serde_json::from_slice(raw_manifest_bytes)
        .map_err(|e| ManifestError::MalformedManifest(e.to_string()))?;

    // 6. Enforce schema version
    if manifest.schema != 1 {
        return Err(ManifestError::UnsupportedSchema(manifest.schema));
    }

    // 7. Anti-rollback check: sequence must not be lower than last accepted
    if manifest.sequence < last_accepted_seq {
        return Err(ManifestError::SequenceRollback {
            received: manifest.sequence,
            last_accepted: last_accepted_seq,
        });
    }

    // 8. Filter components by app version compatibility and rollout gate
    let filtered_components: Vec<ComponentManifestEntry> = manifest
        .components
        .into_iter()
        .filter(|comp| {
            if !is_app_version_compatible(
                app_version,
                &comp.min_app_version,
                comp.max_app_version.as_deref(),
            ) {
                return false;
            }
            if !is_in_rollout(
                install_id,
                &comp.component,
                &comp.version,
                comp.rollout_percent,
            ) {
                return false;
            }
            true
        })
        .collect();

    Ok(ComponentManifest {
        schema: manifest.schema,
        sequence: manifest.sequence,
        generated_at: manifest.generated_at,
        channel: manifest.channel,
        components: filtered_components,
    })
}

/// Verifies manifest against the active `LocalStore`, automatically checking against and
/// persisting the monotonic sequence number to prevent rollback attacks.
pub fn verify_and_accept_manifest(
    raw_manifest_bytes: &[u8],
    raw_sig_bytes: &[u8],
    app_version: &str,
    store: &crate::store::LocalStore,
    extra_keys: Option<&[ManifestPublicKey]>,
) -> Result<ComponentManifest, ManifestError> {
    let last_seq = store
        .get_manifest_sequence()
        .map_err(|e| ManifestError::MalformedManifest(e.to_string()))?;
    let install_id = store
        .get_or_create_install_id()
        .map_err(|e| ManifestError::MalformedManifest(e.to_string()))?;

    let verified = verify_manifest(
        raw_manifest_bytes,
        raw_sig_bytes,
        app_version,
        &install_id,
        last_seq,
        extra_keys,
    )?;

    // Persist new sequence
    store
        .set_manifest_sequence(verified.sequence)
        .map_err(|e| ManifestError::MalformedManifest(e.to_string()))?;

    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn test_verify_manifest_success_and_rejections() {
        // Generate a deterministic test keypair
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let pub_hex = hex::encode(verifying_key.as_bytes());

        let extra_keys = [ManifestPublicKey {
            kid: "dev-test-key-1",
            public_key_hex: Box::leak(pub_hex.into_boxed_str()),
        }];

        let raw_manifest = br#"{
            "schema": 1,
            "sequence": 10,
            "generated_at": "2026-11-24T12:00:00Z",
            "channel": "stable",
            "components": [
                {
                    "component": "yt-dlp",
                    "version": "2026.10.26",
                    "platform": "windows-x86_64",
                    "url": "https://cdn.clipperhelper.com/yt-dlp.zip",
                    "size_bytes": 1000,
                    "sha256": "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
                    "archive": "none",
                    "files": [],
                    "min_app_version": "0.1.0",
                    "max_app_version": null,
                    "rollout_percent": 100
                }
            ]
        }"#;

        use ed25519_dalek::Signer;
        let sig = signing_key.sign(raw_manifest);
        let sig_json = format!(
            r#"{{"kid": "dev-test-key-1", "signature": "{}"}}"#,
            hex::encode(sig.to_bytes())
        );

        // 1. Success verification
        let res = verify_manifest(
            raw_manifest,
            sig_json.as_bytes(),
            "0.1.0",
            "install-id-1",
            5,
            Some(&extra_keys),
        );
        assert!(res.is_ok());
        let verified = res.unwrap();
        assert_eq!(verified.sequence, 10);
        assert_eq!(verified.components.len(), 1);

        // 2. Reject bad signature (tampered bytes)
        let tampered = br#"{"schema": 1, "sequence": 10, "components": []}"#;
        let res_bad_sig = verify_manifest(
            tampered,
            sig_json.as_bytes(),
            "0.1.0",
            "install-id-1",
            5,
            Some(&extra_keys),
        );
        assert!(matches!(
            res_bad_sig.unwrap_err(),
            ManifestError::BadSignature(_)
        ));

        // 3. Reject unknown kid
        let sig_unknown_kid = format!(
            r#"{{"kid": "unknown-kid-99", "signature": "{}"}}"#,
            hex::encode(sig.to_bytes())
        );
        let res_unknown = verify_manifest(
            raw_manifest,
            sig_unknown_kid.as_bytes(),
            "0.1.0",
            "install-id-1",
            5,
            Some(&extra_keys),
        );
        assert!(matches!(
            res_unknown.unwrap_err(),
            ManifestError::UnknownKeyId(_)
        ));

        // 4. Reject anti-rollback (sequence lower than last accepted)
        let res_rollback = verify_manifest(
            raw_manifest,
            sig_json.as_bytes(),
            "0.1.0",
            "install-id-1",
            15, // last accepted is 15 > received 10
            Some(&extra_keys),
        );
        assert!(matches!(
            res_rollback.unwrap_err(),
            ManifestError::SequenceRollback {
                received: 10,
                last_accepted: 15
            }
        ));

        // 5. Version filter: app version 0.0.9 < min_app_version 0.1.0 filters out component
        let res_filtered = verify_manifest(
            raw_manifest,
            sig_json.as_bytes(),
            "0.0.9",
            "install-id-1",
            5,
            Some(&extra_keys),
        )
        .unwrap();
        assert_eq!(res_filtered.components.len(), 0);
    }

    #[test]
    fn test_verify_and_accept_manifest_persists_sequence() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("clipper.db");
        let store = crate::store::LocalStore::open(&db_path).unwrap();

        let signing_key = SigningKey::from_bytes(&[8u8; 32]);
        let pub_hex = hex::encode(signing_key.verifying_key().as_bytes());
        let extra_keys = [ManifestPublicKey {
            kid: "dev-test-key-store",
            public_key_hex: Box::leak(pub_hex.into_boxed_str()),
        }];

        // Sequence 10
        let manifest_10 = br#"{"schema": 1, "sequence": 10, "generated_at": "2026-11-24T12:00:00Z", "channel": "dev", "components": []}"#;
        use ed25519_dalek::Signer;
        let sig_10 = signing_key.sign(manifest_10);
        let sig_10_json = format!(
            r#"{{"kid": "dev-test-key-store", "signature": "{}"}}"#,
            hex::encode(sig_10.to_bytes())
        );

        let res = verify_and_accept_manifest(
            manifest_10,
            sig_10_json.as_bytes(),
            "0.1.0",
            &store,
            Some(&extra_keys),
        );
        assert!(res.is_ok());
        assert_eq!(store.get_manifest_sequence().unwrap(), 10);

        // Sequence 12
        let manifest_12 = br#"{"schema": 1, "sequence": 12, "generated_at": "2026-11-24T12:00:00Z", "channel": "dev", "components": []}"#;
        let sig_12 = signing_key.sign(manifest_12);
        let sig_12_json = format!(
            r#"{{"kid": "dev-test-key-store", "signature": "{}"}}"#,
            hex::encode(sig_12.to_bytes())
        );

        let res_12 = verify_and_accept_manifest(
            manifest_12,
            sig_12_json.as_bytes(),
            "0.1.0",
            &store,
            Some(&extra_keys),
        );
        assert!(res_12.is_ok());
        assert_eq!(store.get_manifest_sequence().unwrap(), 12);

        // Rollback attempt with sequence 10: must be rejected!
        let res_rollback = verify_and_accept_manifest(
            manifest_10,
            sig_10_json.as_bytes(),
            "0.1.0",
            &store,
            Some(&extra_keys),
        );
        assert!(matches!(
            res_rollback.unwrap_err(),
            ManifestError::SequenceRollback {
                received: 10,
                last_accepted: 12
            }
        ));
        assert_eq!(store.get_manifest_sequence().unwrap(), 12);
    }
}
