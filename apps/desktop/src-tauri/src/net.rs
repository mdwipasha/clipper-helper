//! Network security, host allowlists, and HTTP client infrastructure (ARCHITECTURE §4, §10; SECURITY §9).

use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum NetError {
    #[error("Network offline or unreachable: {0}")]
    Offline(String),
    #[error("Network timeout: {0}")]
    Timeout(String),
    #[error("Invalid URL: {0}")]
    UrlInvalid(String),
    #[error("Host is not allowlisted: {0}")]
    HostNotAllowed(String),
    #[error("Non-HTTPS URL is forbidden: {0}")]
    HttpsRequired(String),
    #[error("Redirect to forbidden host or protocol: {0}")]
    RedirectForbidden(String),
    #[error("HTTP error {0}: {1}")]
    HttpError(u16, String),
    #[error("Downloaded payload exceeds declared size: received {actual} bytes, expected {declared} bytes")]
    SizeExceeded { actual: u64, declared: u64 },
    #[error("Streamed payload size mismatch: received {actual} bytes, expected {declared} bytes")]
    SizeMismatch { actual: u64, declared: u64 },
    #[error("Integrity verification failed for '{0}': expected {1}, got {2}")]
    VerificationFailed(String, String, String),
    #[error("Disk full: {0}")]
    DiskFull(String),
    #[error("Disk low: {0}")]
    DiskLow(String),
    #[error("Component download is already in progress: {0}")]
    AlreadyDownloading(String),
    #[error("Download was cancelled")]
    Cancelled,
    #[error("IO error during transfer: {0}")]
    Io(String),
}

impl NetError {
    pub fn error_code(&self) -> &'static str {
        match self {
            NetError::Timeout(_) => "E_NET_TIMEOUT",
            NetError::Offline(_) => "E_NET_OFFLINE",
            NetError::DiskFull(_) => "E_DISK_FULL",
            NetError::DiskLow(_) => "E_DISK_LOW",
            NetError::VerificationFailed(comp, _, _) => {
                if comp.ends_with(".bin") {
                    "E_MODEL_DOWNLOAD_FAILED"
                } else {
                    "E_VERIFY_FAILED"
                }
            }
            NetError::SizeExceeded { .. } | NetError::SizeMismatch { .. } => "E_VERIFY_FAILED",
            _ => "E_NET_OFFLINE",
        }
    }
}

/// Allowlisted hosts for manifest and component artifact downloads.
const ALLOWED_HOST_SUFFIXES: &[&str] = &[
    "clipperhelper.com",
    "r2.cloudflarestorage.com",
    "github.com",
    "githubusercontent.com",
    "huggingface.co",
];

/// Checks if a hostname is permitted by the security host allowlist.
pub fn is_host_allowed(host: &str) -> bool {
    let lower = host.to_lowercase();

    #[cfg(any(debug_assertions, test))]
    {
        if lower == "127.0.0.1" || lower == "localhost" {
            return true;
        }
    }

    for suffix in ALLOWED_HOST_SUFFIXES {
        if lower == *suffix || lower.ends_with(&format!(".{}", suffix)) {
            return true;
        }
    }
    false
}

/// Validates that a URL satisfies HTTPS-only (except loopback in dev/test) and host allowlist.
pub fn validate_download_url(url_str: &str) -> Result<reqwest::Url, NetError> {
    let url = reqwest::Url::parse(url_str).map_err(|e| NetError::UrlInvalid(e.to_string()))?;

    let host = url
        .host_str()
        .ok_or_else(|| NetError::UrlInvalid("URL has no host".to_string()))?;

    if url.scheme() != "https" {
        #[cfg(any(debug_assertions, test))]
        {
            let is_loopback = host == "127.0.0.1" || host == "localhost";
            if is_loopback && url.scheme() == "http" {
                // Permitted in debug/test for local dev-server
                return Ok(url);
            }
        }
        return Err(NetError::HttpsRequired(url_str.to_string()));
    }

    if !is_host_allowed(host) {
        return Err(NetError::HostNotAllowed(host.to_string()));
    }

    Ok(url)
}

/// Creates a hardened `reqwest::Client` with rustls TLS, strict redirect filtering, and timeouts.
pub fn create_secure_client() -> Result<reqwest::Client, NetError> {
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("Too many redirects (max 5)");
        }
        let target_url = attempt.url().as_str();
        if let Err(err) = validate_download_url(target_url) {
            return attempt.error(format!("Redirect rejected by security guard: {}", err));
        }
        attempt.follow()
    });

    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .redirect(redirect_policy)
        .use_rustls_tls()
        .build()
        .map_err(|e| NetError::Offline(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_allowlist_validation() {
        // Valid official hosts
        assert!(validate_download_url("https://cdn.clipperhelper.com/yt-dlp.zip").is_ok());
        assert!(
            validate_download_url("https://pub-123.r2.cloudflarestorage.com/model.bin").is_ok()
        );
        assert!(
            validate_download_url("https://github.com/yt-dlp/yt-dlp/releases/download/v1.exe")
                .is_ok()
        );
        assert!(validate_download_url("https://objects.githubusercontent.com/archive.zip").is_ok());
        assert!(validate_download_url(
            "https://huggingface.co/ggerganov/whisper.cpp/raw/main/model.bin"
        )
        .is_ok());

        // Local loopback in dev/test
        assert!(validate_download_url("http://127.0.0.1:8080/manifest.json").is_ok());

        // Disallowed external HTTP
        assert!(matches!(
            validate_download_url("http://cdn.clipperhelper.com/manifest.json").unwrap_err(),
            NetError::HttpsRequired(_)
        ));

        // Disallowed hosts
        assert!(matches!(
            validate_download_url("https://evil-mirror.com/yt-dlp.zip").unwrap_err(),
            NetError::HostNotAllowed(_)
        ));
        assert!(matches!(
            validate_download_url("https://malicious.org/model.bin").unwrap_err(),
            NetError::HostNotAllowed(_)
        ));
    }
}
