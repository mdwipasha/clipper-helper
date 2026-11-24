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
    #[error("Manifest verification failed: {0}")]
    ManifestInvalid(String),
    #[error("Manifest sequence rollback rejected: received {received}, expected >= {floor}")]
    ManifestStale { received: u64, floor: u64 },
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
            NetError::ManifestInvalid(_) => "E_MANIFEST_INVALID",
            NetError::ManifestStale { .. } => "E_MANIFEST_STALE",
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

/// Maps a reqwest::Error to the appropriate NetError taxonomy variant.
pub fn map_reqwest_error(e: reqwest::Error) -> NetError {
    if e.is_timeout() {
        NetError::Timeout(e.to_string())
    } else if e.is_connect() {
        NetError::Offline(e.to_string())
    } else if let Some(status) = e.status() {
        NetError::HttpError(status.as_u16(), status.to_string())
    } else {
        NetError::Offline(e.to_string())
    }
}

/// Primary owned CDN host for official binary and model artifact distribution (SECURITY §11).
pub const PRIMARY_CDN_HOST: &str = "cdn.clipperhelper.com";

/// Allowlisted hosts for manifest and component artifact downloads.
/// Includes redirect destinations like release-assets.githubusercontent.com.
const ALLOWED_HOST_SUFFIXES: &[&str] = &[
    "clipperhelper.com",
    "r2.cloudflarestorage.com",
    "github.com",
    "githubusercontent.com",
    "release-assets.githubusercontent.com",
    "huggingface.co",
];

/// Checks if a hostname is permitted strictly under production release rules.
pub fn is_host_allowed_release(host: &str) -> bool {
    let lower = host.to_lowercase();
    for suffix in ALLOWED_HOST_SUFFIXES {
        if lower == *suffix || lower.ends_with(&format!(".{}", suffix)) {
            return true;
        }
    }
    false
}

/// Checks if a hostname is permitted by the security host allowlist.
pub fn is_host_allowed(host: &str) -> bool {
    #[cfg(debug_assertions)]
    {
        let lower = host.to_lowercase();
        if lower == "127.0.0.1" || lower == "localhost" {
            return true;
        }
    }

    is_host_allowed_release(host)
}

/// Internal URL validator parameterized by loopback permission.
pub(crate) fn validate_download_url_internal(
    url_str: &str,
    allow_loopback: bool,
) -> Result<reqwest::Url, NetError> {
    let url = reqwest::Url::parse(url_str).map_err(|e| NetError::UrlInvalid(e.to_string()))?;

    let host = url
        .host_str()
        .ok_or_else(|| NetError::UrlInvalid("URL has no host".to_string()))?;

    let lower = host.to_lowercase();
    let is_loopback = lower == "127.0.0.1" || lower == "localhost";

    if url.scheme() != "https" {
        if allow_loopback && is_loopback && url.scheme() == "http" {
            return Ok(url);
        }
        return Err(NetError::HttpsRequired(url_str.to_string()));
    }

    if allow_loopback && is_loopback {
        return Ok(url);
    }

    if !is_host_allowed_release(&lower) {
        return Err(NetError::HostNotAllowed(host.to_string()));
    }

    Ok(url)
}

/// Validates that a URL satisfies HTTPS-only (except loopback in debug builds) and host allowlist.
pub fn validate_download_url(url_str: &str) -> Result<reqwest::Url, NetError> {
    #[cfg(debug_assertions)]
    let allow_loopback = true;
    #[cfg(not(debug_assertions))]
    let allow_loopback = false;

    validate_download_url_internal(url_str, allow_loopback)
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

        // Local loopback in debug builds only
        #[cfg(debug_assertions)]
        assert!(validate_download_url("http://127.0.0.1:8080/manifest.json").is_ok());

        // Release-assets on githubusercontent
        assert!(validate_download_url(
            "https://release-assets.githubusercontent.com/123/asset.zip"
        )
        .is_ok());

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

    #[test]
    fn test_release_validation_rejects_loopback() {
        // In release mode (allow_loopback = false), all loopback URLs must be rejected
        let res_http = validate_download_url_internal("http://127.0.0.1:8080/manifest.json", false);
        assert!(matches!(res_http.unwrap_err(), NetError::HttpsRequired(_)));

        let res_https =
            validate_download_url_internal("https://127.0.0.1:8080/manifest.json", false);
        assert!(matches!(
            res_https.unwrap_err(),
            NetError::HostNotAllowed(_)
        ));

        let res_localhost =
            validate_download_url_internal("http://localhost:8080/manifest.json", false);
        assert!(matches!(
            res_localhost.unwrap_err(),
            NetError::HttpsRequired(_)
        ));

        // Legitimate CDN and GitHub assets must still be accepted in release
        assert!(
            validate_download_url_internal("https://cdn.clipperhelper.com/yt-dlp.zip", false)
                .is_ok()
        );
        assert!(validate_download_url_internal(
            "https://release-assets.githubusercontent.com/asset.zip",
            false
        )
        .is_ok());
    }

    #[test]
    fn test_error_code_mapping() {
        assert_eq!(
            NetError::Offline("connection refused".into()).error_code(),
            "E_NET_OFFLINE"
        );
        assert_eq!(
            NetError::Timeout("timed out".into()).error_code(),
            "E_NET_TIMEOUT"
        );
        assert_eq!(
            NetError::ManifestInvalid("bad signature".into()).error_code(),
            "E_MANIFEST_INVALID"
        );
        assert_eq!(
            NetError::ManifestStale {
                received: 1,
                floor: 2
            }
            .error_code(),
            "E_MANIFEST_STALE"
        );
        assert_eq!(
            NetError::DiskFull("0 bytes free".into()).error_code(),
            "E_DISK_FULL"
        );
        assert_eq!(
            NetError::DiskLow("10 MB free".into()).error_code(),
            "E_DISK_LOW"
        );
        assert_eq!(
            NetError::VerificationFailed("whisper.bin".into(), "expected".into(), "actual".into())
                .error_code(),
            "E_MODEL_DOWNLOAD_FAILED"
        );
        assert_eq!(
            NetError::VerificationFailed("ffmpeg.exe".into(), "expected".into(), "actual".into())
                .error_code(),
            "E_VERIFY_FAILED"
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn test_release_profile_rejects_loopback_and_no_debug_allowlist() {
        assert!(!is_host_allowed("127.0.0.1"));
        assert!(!is_host_allowed("localhost"));
        assert!(!is_host_allowed("LOCALHOST"));

        assert!(matches!(
            validate_download_url("http://127.0.0.1:8080/manifest.json").unwrap_err(),
            NetError::HttpsRequired(_)
        ));
        assert!(matches!(
            validate_download_url("https://127.0.0.1:8080/manifest.json").unwrap_err(),
            NetError::HostNotAllowed(_)
        ));
        assert!(matches!(
            validate_download_url("http://localhost:8080/manifest.json").unwrap_err(),
            NetError::HttpsRequired(_)
        ));
        assert!(matches!(
            validate_download_url("https://localhost:8080/manifest.json").unwrap_err(),
            NetError::HostNotAllowed(_)
        ));
    }

    #[tokio::test]
    async fn test_reqwest_connect_error_maps_to_net_offline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener); // Port is now closed

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("build client");

        let err = client
            .get(format!("http://127.0.0.1:{}/manifest.json", port))
            .send()
            .await
            .expect_err("closed port must fail to connect");

        assert!(
            err.is_connect(),
            "expected reqwest connect error: {:?}",
            err
        );
        let mapped = map_reqwest_error(err);
        assert!(matches!(mapped, NetError::Offline(_)));
        assert_eq!(mapped.error_code(), "E_NET_OFFLINE");
    }

    #[tokio::test]
    async fn test_reqwest_timeout_maps_to_net_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stalling listener");
        let port = listener.local_addr().expect("local addr").port();

        std::thread::spawn(move || {
            if let Ok((_socket, _)) = listener.accept() {
                // Hold connection open without sending any response bytes
                std::thread::sleep(Duration::from_secs(2));
            }
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(150))
            .build()
            .expect("build client");

        let err = client
            .get(format!("http://127.0.0.1:{}/manifest.json", port))
            .send()
            .await
            .expect_err("stalled server must trigger timeout");

        assert!(
            err.is_timeout(),
            "expected reqwest timeout error: {:?}",
            err
        );
        let mapped = map_reqwest_error(err);
        assert!(matches!(mapped, NetError::Timeout(_)));
        assert_eq!(mapped.error_code(), "E_NET_TIMEOUT");
    }
}
