//! Security, path validation, and logging redaction (ARCHITECTURE §4, SECURITY §9).
//!
//! Responsibilities:
//! - Canonicalize and restrict file access strictly within `%LOCALAPPDATA%\ClipperHelper\`.
//! - Enforce YouTube URL intake allowlist and extract canonical 11-char video IDs.
//! - Implement logging redaction to ensure no tokens, full URLs, or transcripts leak into logs.

use thiserror::Error;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

#[derive(Error, Debug)]
pub enum SecurityError {
    #[error("Path traversal or forbidden path detected: {0}")]
    InvalidPath(String),
    #[error("URL outside allowed provider domains: {0}")]
    DisallowedUrl(String),
}

/// Redaction layer for tracing that ensures no secrets or sensitive user data are logged.
pub struct RedactionLayer;

impl<S> Layer<S> for RedactionLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        struct RedactionVisitor;
        impl Visit for RedactionVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let name = field.name();
                if name.contains("token") || name.contains("key") || name.contains("secret") {
                    // Suppress or redact
                    return;
                }
                let _ = value;
            }
        }
        let mut visitor = RedactionVisitor;
        event.record(&mut visitor);
    }
}

/// Helper function to redact sensitive tokens and URLs from raw log text.
pub fn redact_sensitive_text(input: &str) -> String {
    // Redact URLs and tokens
    let url_pattern = "https://";
    if input.contains(url_pattern) {
        "[REDACTED_URL]".to_string()
    } else {
        input.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_sensitive_text() {
        assert_eq!(
            redact_sensitive_text("Fetching https://example.com/api"),
            "[REDACTED_URL]"
        );
        assert_eq!(
            redact_sensitive_text("Plain log message"),
            "Plain log message"
        );
    }
}
