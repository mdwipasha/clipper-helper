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
        match self {
            ManifestError::SequenceRollback { .. } => "E_MANIFEST_STALE",
            _ => "E_MANIFEST_INVALID",
        }
    }
}

/// Formats a component or model version consistently for UI, Diagnostics reports, and errors:
/// never prepends `v` when the trimmed string already begins with an ASCII letter (`v`, `n`, `silero-...`, `whisper-...`).
pub fn format_component_version(version: &str) -> String {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        trimmed.to_string()
    } else {
        format!("v{}", trimmed)
    }
}

const fn parse_u64_const(s: &str) -> u64 {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        panic!("CLIPPER_MIN_MANIFEST_SEQUENCE must not be empty");
    }
    let mut res: u64 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            panic!("CLIPPER_MIN_MANIFEST_SEQUENCE must be a positive integer");
        }
        res = res * 10 + (b - b'0') as u64;
        i += 1;
    }
    if res < 1 {
        panic!("CLIPPER_MIN_MANIFEST_SEQUENCE must be >= 1");
    }
    res
}

/// Build-time floor for manifest sequence number (injected from CLIPPER_MIN_MANIFEST_SEQUENCE).
/// Prevents replay and rollback attacks even if the local database is reset, recreated,
/// or recovered from corruption (ARCHITECTURE §9, §13).
pub const MIN_MANIFEST_SEQUENCE: u64 = parse_u64_const(env!("CLIPPER_MIN_MANIFEST_SEQUENCE"));

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
    #[serde(default)]
    pub mirrors: Vec<String>,
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
    let effective_floor = std::cmp::max(last_seq, MIN_MANIFEST_SEQUENCE);
    let install_id = store
        .get_or_create_install_id()
        .map_err(|e| ManifestError::MalformedManifest(e.to_string()))?;

    let verified = verify_manifest(
        raw_manifest_bytes,
        raw_sig_bytes,
        app_version,
        &install_id,
        effective_floor,
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

    #[test]
    fn test_anti_rollback_floor_survives_db_reset() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("clipper.db");
        let store = crate::store::LocalStore::open(&db_path).unwrap();

        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let pub_hex = hex::encode(signing_key.verifying_key().as_bytes());
        let extra_keys = [ManifestPublicKey {
            kid: "dev-floor-test-key",
            public_key_hex: Box::leak(pub_hex.into_boxed_str()),
        }];

        // Manifest with sequence 0 (< MIN_MANIFEST_SEQUENCE which is 1)
        let manifest_0 = br#"{"schema": 1, "sequence": 0, "generated_at": "2026-11-24T12:00:00Z", "channel": "dev", "components": []}"#;
        use ed25519_dalek::Signer;
        let sig_0 = signing_key.sign(manifest_0);
        let sig_0_json = format!(
            r#"{{"kid": "dev-floor-test-key", "signature": "{}"}}"#,
            hex::encode(sig_0.to_bytes())
        );

        // Even with a freshly initialized store (persisted seq = 0), sequence 0 must be rejected!
        let res_0 = verify_and_accept_manifest(
            manifest_0,
            sig_0_json.as_bytes(),
            "0.1.0",
            &store,
            Some(&extra_keys),
        );
        assert!(matches!(
            res_0.unwrap_err(),
            ManifestError::SequenceRollback {
                received: 0,
                last_accepted: 1
            }
        ));
    }

    #[test]
    fn test_wrong_key_and_unknown_kid_distinct_cases() {
        use ed25519_dalek::Signer;

        let trusted_key = SigningKey::from_bytes(&[11u8; 32]);
        let wrong_key = SigningKey::from_bytes(&[22u8; 32]);
        let pub_hex = hex::encode(trusted_key.verifying_key().as_bytes());

        let extra_keys = [ManifestPublicKey {
            kid: "trusted-kid-1",
            public_key_hex: Box::leak(pub_hex.into_boxed_str()),
        }];

        let raw_manifest = br#"{"schema": 1, "sequence": 2, "generated_at": "2026-11-24T12:00:00Z", "channel": "stable", "components": []}"#;

        // Case A: Wrong key (valid kid known to verifier, but signed by a different private key)
        let wrong_sig = wrong_key.sign(raw_manifest);
        let wrong_sig_json = format!(
            r#"{{"kid": "trusted-kid-1", "signature": "{}"}}"#,
            hex::encode(wrong_sig.to_bytes())
        );
        let err_wrong_key = verify_manifest(
            raw_manifest,
            wrong_sig_json.as_bytes(),
            "0.1.0",
            "install-1",
            1,
            Some(&extra_keys),
        )
        .unwrap_err();
        assert!(
            matches!(err_wrong_key, ManifestError::BadSignature(_)),
            "expected BadSignature for wrong key with valid kid, got {:?}",
            err_wrong_key
        );
        assert_eq!(err_wrong_key.error_code(), "E_MANIFEST_INVALID");

        // Case B: Unknown kid (signed cleanly, but kid is not in embedded/extra keyrings)
        let valid_sig = trusted_key.sign(raw_manifest);
        let unknown_kid_json = format!(
            r#"{{"kid": "unrecognized-kid-99", "signature": "{}"}}"#,
            hex::encode(valid_sig.to_bytes())
        );
        let err_unknown_kid = verify_manifest(
            raw_manifest,
            unknown_kid_json.as_bytes(),
            "0.1.0",
            "install-1",
            1,
            Some(&extra_keys),
        )
        .unwrap_err();
        assert!(
            matches!(err_unknown_kid, ManifestError::UnknownKeyId(ref k) if k == "unrecognized-kid-99"),
            "expected UnknownKeyId for unknown kid, got {:?}",
            err_unknown_kid
        );
        assert_eq!(err_unknown_kid.error_code(), "E_MANIFEST_INVALID");
    }

    #[test]
    fn test_schema_and_malformed_json_map_to_manifest_invalid() {
        use ed25519_dalek::Signer;

        let signing_key = SigningKey::from_bytes(&[12u8; 32]);
        let pub_hex = hex::encode(signing_key.verifying_key().as_bytes());
        let extra_keys = [ManifestPublicKey {
            kid: "schema-test-kid",
            public_key_hex: Box::leak(pub_hex.into_boxed_str()),
        }];

        // Unsupported schema (v99 signed properly -> UnsupportedSchema -> E_MANIFEST_INVALID)
        let bad_schema = br#"{"schema": 99, "sequence": 5, "generated_at": "2026-11-24T12:00:00Z", "channel": "stable", "components": []}"#;
        let sig_schema = signing_key.sign(bad_schema);
        let sig_schema_json = format!(
            r#"{{"kid": "schema-test-kid", "signature": "{}"}}"#,
            hex::encode(sig_schema.to_bytes())
        );
        let err_schema = verify_manifest(
            bad_schema,
            sig_schema_json.as_bytes(),
            "0.1.0",
            "install-1",
            1,
            Some(&extra_keys),
        )
        .unwrap_err();
        assert!(matches!(err_schema, ManifestError::UnsupportedSchema(99)));
        assert_eq!(err_schema.error_code(), "E_MANIFEST_INVALID");

        // Malformed manifest JSON (signed properly -> MalformedManifest -> E_MANIFEST_INVALID)
        let bad_json = b"{not valid json";
        let sig_json = signing_key.sign(bad_json);
        let sig_json_str = format!(
            r#"{{"kid": "schema-test-kid", "signature": "{}"}}"#,
            hex::encode(sig_json.to_bytes())
        );
        let err_malformed = verify_manifest(
            bad_json,
            sig_json_str.as_bytes(),
            "0.1.0",
            "install-1",
            1,
            Some(&extra_keys),
        )
        .unwrap_err();
        assert!(matches!(err_malformed, ManifestError::MalformedManifest(_)));
        assert_eq!(err_malformed.error_code(), "E_MANIFEST_INVALID");

        // Sequence rollback error code maps to E_MANIFEST_STALE
        let err_stale = ManifestError::SequenceRollback {
            received: 2,
            last_accepted: 5,
        };
        assert_eq!(err_stale.error_code(), "E_MANIFEST_STALE");
    }

    #[test]
    fn test_app_version_and_rollout_mismatch_skip_entries_without_error() {
        use ed25519_dalek::Signer;

        let signing_key = SigningKey::from_bytes(&[13u8; 32]);
        let pub_hex = hex::encode(signing_key.verifying_key().as_bytes());
        let extra_keys = [ManifestPublicKey {
            kid: "gating-test-kid",
            public_key_hex: Box::leak(pub_hex.into_boxed_str()),
        }];

        let manifest_bytes = br#"{
            "schema": 1,
            "sequence": 5,
            "generated_at": "2026-11-24T12:00:00Z",
            "channel": "stable",
            "components": [
                {
                    "component": "yt-dlp",
                    "version": "2026.10.26",
                    "platform": "windows-x86_64",
                    "url": "https://cdn.clipperhelper.com/yt-dlp.exe",
                    "size_bytes": 1000,
                    "sha256": "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
                    "archive": "none",
                    "files": [],
                    "min_app_version": "0.2.0",
                    "max_app_version": "0.5.0",
                    "rollout_percent": 100
                },
                {
                    "component": "qjs",
                    "version": "0.18.0",
                    "platform": "windows-x86_64",
                    "url": "https://cdn.clipperhelper.com/qjs.exe",
                    "size_bytes": 500,
                    "sha256": "ff03398d74ea6e6bd3a8e58058f7003225abcac229d7d57775b7aaae812a49c7",
                    "archive": "none",
                    "files": [],
                    "min_app_version": "0.1.0",
                    "max_app_version": null,
                    "rollout_percent": 0
                }
            ]
        }"#;

        let sig = signing_key.sign(manifest_bytes);
        let sig_json = format!(
            r#"{{"kid": "gating-test-kid", "signature": "{}"}}"#,
            hex::encode(sig.to_bytes())
        );

        // App 0.1.0: yt-dlp skipped (< min 0.2.0), qjs skipped (rollout_percent == 0) -> Ok with 0 components
        let res_too_old = verify_manifest(
            manifest_bytes,
            sig_json.as_bytes(),
            "0.1.0",
            "install-abc",
            1,
            Some(&extra_keys),
        )
        .expect("app-version / rollout mismatch must not return an error");
        assert_eq!(res_too_old.components.len(), 0);

        // App 0.6.0: yt-dlp skipped (> max 0.5.0), qjs skipped (rollout_percent == 0) -> Ok with 0 components
        let res_too_new = verify_manifest(
            manifest_bytes,
            sig_json.as_bytes(),
            "0.6.0",
            "install-abc",
            1,
            Some(&extra_keys),
        )
        .expect("max_app_version mismatch must not return an error");
        assert_eq!(res_too_new.components.len(), 0);

        // App 0.3.0: yt-dlp included, qjs still skipped (0% rollout)
        let res_in_range = verify_manifest(
            manifest_bytes,
            sig_json.as_bytes(),
            "0.3.0",
            "install-abc",
            1,
            Some(&extra_keys),
        )
        .expect("in-range version must succeed");
        assert_eq!(res_in_range.components.len(), 1);
        assert_eq!(res_in_range.components[0].component, "yt-dlp");
    }

    #[test]
    fn test_format_component_version_table() {
        let cases = [
            (
                "n9.0.2-58-g8c7ef49002-20261119",
                "n9.0.2-58-g8c7ef49002-20261119",
            ),
            ("v0.18.0", "v0.18.0"),
            ("1.9.5", "v1.9.5"),
            ("2026.10.26", "v2026.10.26"),
            ("silero-v6.2.0-2aa269b7", "silero-v6.2.0-2aa269b7"),
            ("whisper-small-q5_1-ae85e4a9", "whisper-small-q5_1-ae85e4a9"),
            ("  ", ""),
        ];
        for (raw, expected) in cases {
            assert_eq!(format_component_version(raw), expected, "raw={}", raw);
        }
    }
}
