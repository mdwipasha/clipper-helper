//! License and export ticket verification (ARCHITECTURE §4, SECURITY §5).
//!
//! Responsibilities:
//! - Verify Ed25519 signature of export tickets issued by Supabase Edge Functions.
//! - Enforce ticket expiration, key id (`kid`), device fingerprint, and composition hash matching.
//! - Securely store tokens and machine credentials in Windows Credential Manager.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum LicenseError {
    #[error("Ticket signature invalid")]
    InvalidSignature,
    #[error("Ticket has expired")]
    Expired,
    #[error("Composition hash mismatch")]
    HashMismatch,
}

/// Stub license and ticket verification service.
#[derive(Default)]
pub struct LicenseService;

impl LicenseService {
    pub fn new() -> Self {
        Self
    }
}
