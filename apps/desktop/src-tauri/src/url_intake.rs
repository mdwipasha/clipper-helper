//! Pure, strict YouTube single-video URL intake and canonical normalization.
//!
//! Validates pasted URLs against tight host, scheme, path, character, length,
//! and 11-character video ID constraints before any sidecar (`yt-dlp`) is
//! invoked, and builds the canonical `https://www.youtube.com/watch?v=<id>` URL
//! exclusively from the validated ID.

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;
use url::Url;

/// Maximum accepted raw input length before trimming/parsing.
pub const MAX_YOUTUBE_URL_LENGTH: usize = 2048;

/// Upper bound applied to `suggested_start_seconds` extracted from `t` / `start`.
pub const MAX_SUGGESTED_START_SECONDS: u32 = 86_400;

const ALLOWED_HOSTS: [&str; 4] = [
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "youtu.be",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum UrlInvalidReason {
    Empty,
    TooLong,
    UnsafeCharacters,
    InvalidSyntax,
    UnsupportedScheme,
    UserinfoNotAllowed,
    PortNotAllowed,
    HostNotAllowed,
    PlaylistOrChannelNotSupported,
    DoubleEncodedOrInvalidId,
    MissingOrInvalidVideoId,
}

impl UrlInvalidReason {
    /// Returns the i18n message key suffix for this invalid-URL reason.
    pub fn message_key(self) -> &'static str {
        match self {
            Self::PlaylistOrChannelNotSupported => "err.E_YT_URL_INVALID.collection",
            Self::TooLong => "err.E_YT_URL_INVALID.too_long",
            Self::HostNotAllowed | Self::UnsupportedScheme => "err.E_YT_URL_INVALID.host",
            _ => "err.E_YT_URL_INVALID",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlIntakeError {
    #[error("[E_YT_URL_INVALID] {message}")]
    Invalid {
        reason: UrlInvalidReason,
        message: String,
    },
}

impl UrlIntakeError {
    fn new(reason: UrlInvalidReason, message: impl Into<String>) -> Self {
        Self::Invalid {
            reason,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        "E_YT_URL_INVALID"
    }

    pub fn reason(&self) -> UrlInvalidReason {
        match self {
            Self::Invalid { reason, .. } => *reason,
        }
    }

    pub fn message_key(&self) -> &'static str {
        self.reason().message_key()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ParsedYouTubeUrl {
    /// Validated 11-character YouTube video ID (`^[A-Za-z0-9_-]{11}$`).
    pub video_id: String,
    /// Canonical watch URL constructed solely from `video_id`.
    pub canonical_url: String,
    /// Optional start offset extracted from `t` or `start`, capped at `MAX_SUGGESTED_START_SECONDS`.
    pub suggested_start_seconds: Option<u32>,
}

/// Validates and normalizes a pasted YouTube single-video URL.
pub fn parse_youtube_url(raw: &str) -> Result<ParsedYouTubeUrl, UrlIntakeError> {
    if raw.chars().any(is_forbidden_control_or_invisible_char) {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::UnsafeCharacters,
            "URL contains forbidden control, zero-width, or bidirectional characters.",
        ));
    }

    // Trim leading/trailing ordinary spaces (like copy-paste padding) before length/internal-space checks.
    let trimmed = raw.trim_matches(' ');
    if trimmed.is_empty() {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::Empty,
            "URL is empty.",
        ));
    }

    if trimmed.len() > MAX_YOUTUBE_URL_LENGTH {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::TooLong,
            format!(
                "URL exceeds maximum length of {} characters.",
                MAX_YOUTUBE_URL_LENGTH
            ),
        ));
    }

    if trimmed.chars().any(char::is_whitespace) {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::UnsafeCharacters,
            "URL contains internal whitespace.",
        ));
    }

    if !trimmed.is_ascii() {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::UnsafeCharacters,
            "URL must contain only ASCII characters.",
        ));
    }

    if trimmed.contains('\\') {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::InvalidSyntax,
            "Backslashes are not allowed in URLs.",
        ));
    }

    let (scheme_raw, after_scheme) = trimmed.split_once("://").ok_or_else(|| {
        UrlIntakeError::new(
            UrlInvalidReason::InvalidSyntax,
            "URL must include http:// or https:// scheme.",
        )
    })?;

    let scheme_lower = scheme_raw.to_ascii_lowercase();
    if scheme_lower != "https" && scheme_lower != "http" {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::UnsupportedScheme,
            format!("Unsupported URL scheme: {}", scheme_raw),
        ));
    }

    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let raw_authority = &after_scheme[..authority_end];

    if raw_authority.is_empty() {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::InvalidSyntax,
            "URL is missing a host.",
        ));
    }

    if raw_authority.contains('@') {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::UserinfoNotAllowed,
            "Userinfo (@) is not allowed in YouTube URLs.",
        ));
    }

    if raw_authority.contains(':') || raw_authority.contains('[') || raw_authority.contains(']') {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::PortNotAllowed,
            "Explicit ports or IP literals are not allowed in YouTube URLs.",
        ));
    }

    if raw_authority.contains('%') {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::HostNotAllowed,
            "Percent-encoding in host is not allowed.",
        ));
    }

    let host_lower = raw_authority.to_ascii_lowercase();
    if host_lower.contains("xn--") || !ALLOWED_HOSTS.contains(&host_lower.as_str()) {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::HostNotAllowed,
            format!("Host '{}' is not an allowed YouTube domain.", raw_authority),
        ));
    }

    let parsed = Url::parse(trimmed).map_err(|_| {
        UrlIntakeError::new(UrlInvalidReason::InvalidSyntax, "Malformed YouTube URL.")
    })?;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::UserinfoNotAllowed,
            "Userinfo is not allowed in YouTube URLs.",
        ));
    }

    if parsed.port().is_some() {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::PortNotAllowed,
            "Ports are not allowed in YouTube URLs.",
        ));
    }

    let parsed_host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if parsed_host != host_lower {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::HostNotAllowed,
            "Host mismatch detected.",
        ));
    }

    // Reject double-encoded percent sequences anywhere in path/query.
    if parsed.path().to_ascii_lowercase().contains("%25")
        || parsed
            .query()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("%25")
    {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::DoubleEncodedOrInvalidId,
            "Double-encoded sequences are not allowed.",
        ));
    }

    let raw_video_id = extract_video_id(&parsed, &host_lower)?;
    if !is_valid_youtube_video_id(&raw_video_id) {
        return Err(UrlIntakeError::new(
            UrlInvalidReason::MissingOrInvalidVideoId,
            "Video ID must be 11 characters from [A-Za-z0-9_-].",
        ));
    }

    let canonical_url = format!("https://www.youtube.com/watch?v={}", raw_video_id);
    let suggested_start_seconds = extract_suggested_start_seconds(&parsed);

    Ok(ParsedYouTubeUrl {
        video_id: raw_video_id,
        canonical_url,
        suggested_start_seconds,
    })
}

fn is_forbidden_control_or_invisible_char(c: char) -> bool {
    if c.is_control() {
        return true;
    }
    matches!(
        c,
        '\u{00AD}' // Soft hyphen
            | '\u{061C}' // Arabic letter mark
            | '\u{180E}' // Mongolian vowel separator
            | '\u{200B}'..='\u{200F}' // ZWSP, ZWNJ, ZWJ, LRM, RLM
            | '\u{202A}'..='\u{202E}' // Bidi embedding/override/PDF
            | '\u{2060}'..='\u{2064}' // Word joiner / invisible operators
            | '\u{2066}'..='\u{206F}' // Bidi isolates / deprecated formatters
            | '\u{FEFF}' // BOM / ZWNBSP
    )
}

fn is_valid_youtube_video_id(candidate: &str) -> bool {
    candidate.len() == 11
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn extract_raw_query_values<'a>(raw_query: &'a str, key: &str) -> Vec<&'a str> {
    raw_query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| match pair.split_once('=') {
            Some((k, v)) if k == key => Some(v),
            Some(_) => None,
            None if pair == key => Some(""),
            None => None,
        })
        .collect()
}

fn extract_video_id(parsed: &Url, host_lower: &str) -> Result<String, UrlIntakeError> {
    let raw_path = parsed.path();
    let segments: Vec<&str> = raw_path
        .trim_end_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    if host_lower == "youtu.be" {
        if segments.len() != 1 {
            return Err(UrlIntakeError::new(
                UrlInvalidReason::MissingOrInvalidVideoId,
                "youtu.be URL must contain exactly one video ID path segment.",
            ));
        }
        let seg = segments[0];
        if seg.contains('%') {
            return Err(UrlIntakeError::new(
                UrlInvalidReason::DoubleEncodedOrInvalidId,
                "Encoded video IDs are not allowed.",
            ));
        }
        if !is_valid_youtube_video_id(seg) {
            return Err(UrlIntakeError::new(
                UrlInvalidReason::MissingOrInvalidVideoId,
                "Invalid youtu.be video ID.",
            ));
        }
        return Ok(seg.to_string());
    }

    match segments.as_slice() {
        ["watch"] => {
            let raw_query = parsed.query().unwrap_or_default();
            let raw_vs = extract_raw_query_values(raw_query, "v");
            if raw_vs.is_empty() {
                if !extract_raw_query_values(raw_query, "list").is_empty() {
                    return Err(UrlIntakeError::new(
                        UrlInvalidReason::PlaylistOrChannelNotSupported,
                        "Pure playlist URLs without a video ID (`v=`) are not supported.",
                    ));
                }
                return Err(UrlIntakeError::new(
                    UrlInvalidReason::MissingOrInvalidVideoId,
                    "Watch URL is missing `v=` video ID.",
                ));
            }
            if raw_vs.len() > 1 {
                return Err(UrlIntakeError::new(
                    UrlInvalidReason::MissingOrInvalidVideoId,
                    "Watch URL has multiple `v=` parameters.",
                ));
            }
            let raw_v = raw_vs[0];
            if raw_v.contains('%') {
                return Err(UrlIntakeError::new(
                    UrlInvalidReason::DoubleEncodedOrInvalidId,
                    "Encoded video IDs are not allowed.",
                ));
            }
            if !is_valid_youtube_video_id(raw_v) {
                return Err(UrlIntakeError::new(
                    UrlInvalidReason::MissingOrInvalidVideoId,
                    "Watch `v=` parameter is not a valid 11-character video ID.",
                ));
            }
            Ok(raw_v.to_string())
        }
        ["live" | "shorts" | "embed", seg] => {
            if seg.contains('%') {
                return Err(UrlIntakeError::new(
                    UrlInvalidReason::DoubleEncodedOrInvalidId,
                    "Encoded video IDs are not allowed.",
                ));
            }
            if !is_valid_youtube_video_id(seg) {
                return Err(UrlIntakeError::new(
                    UrlInvalidReason::MissingOrInvalidVideoId,
                    "Path video ID must be 11 characters.",
                ));
            }
            Ok((*seg).to_string())
        }
        ["playlist"] | ["results"] => Err(UrlIntakeError::new(
            UrlInvalidReason::PlaylistOrChannelNotSupported,
            "Playlist, search, and channel URLs are not supported; paste a single video link.",
        )),
        [first, ..] if first.starts_with('@') => Err(UrlIntakeError::new(
            UrlInvalidReason::PlaylistOrChannelNotSupported,
            "Channel handle URLs (@handle) are not supported; paste a single video link.",
        )),
        ["channel" | "c" | "user" | "feed" | "clip", ..] => Err(UrlIntakeError::new(
            UrlInvalidReason::PlaylistOrChannelNotSupported,
            "Channel and feed URLs are not supported; paste a single video link.",
        )),
        _ => Err(UrlIntakeError::new(
            UrlInvalidReason::MissingOrInvalidVideoId,
            "Unsupported YouTube URL path.",
        )),
    }
}

fn extract_suggested_start_seconds(parsed: &Url) -> Option<u32> {
    let mut t_val: Option<String> = None;
    let mut start_val: Option<String> = None;

    for (k, v) in parsed.query_pairs() {
        if k == "t" && t_val.is_none() {
            t_val = Some(v.into_owned());
        } else if k == "start" && start_val.is_none() {
            start_val = Some(v.into_owned());
        }
    }

    t_val
        .as_deref()
        .and_then(parse_time_offset_seconds)
        .or_else(|| start_val.as_deref().and_then(parse_time_offset_seconds))
}

/// Parses `t` / `start` parameter (`"90"`, `"90s"`, `"1m30s"`, `"1h2m3s"`), capped cleanly.
fn parse_time_offset_seconds(raw: &str) -> Option<u32> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    if s.bytes().all(|b| b.is_ascii_digit()) {
        let secs = s
            .parse::<u64>()
            .unwrap_or(MAX_SUGGESTED_START_SECONDS as u64);
        return Some(secs.min(MAX_SUGGESTED_START_SECONDS as u64) as u32);
    }

    let mut total: u64 = 0;
    let mut current_digits = String::new();
    let mut saw_unit = false;
    let mut last_unit_rank = 0u8; // 1=h, 2=m, 3=s

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current_digits.push(ch);
            continue;
        }
        let unit = ch.to_ascii_lowercase();
        let (rank, mult): (u8, u64) = match unit {
            'h' => (1, 3600),
            'm' => (2, 60),
            's' => (3, 1),
            _ => return None,
        };
        if current_digits.is_empty() || rank <= last_unit_rank {
            return None;
        }
        let val = current_digits
            .parse::<u64>()
            .unwrap_or(MAX_SUGGESTED_START_SECONDS as u64);
        total = total.saturating_add(val.saturating_mul(mult));
        current_digits.clear();
        saw_unit = true;
        last_unit_rank = rank;
    }

    if !current_digits.is_empty() || !saw_unit {
        return None;
    }

    Some(total.min(MAX_SUGGESTED_START_SECONDS as u64) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ID: &str = "dQw4w9WgXcQ";
    const EXPECTED_CANONICAL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

    #[test]
    fn test_valid_youtube_urls_table_driven() {
        struct ValidCase {
            name: &'static str,
            input: &'static str,
            expected_id: &'static str,
            expected_start: Option<u32>,
        }

        let cases = [
            ValidCase {
                name: "standard watch https",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "watch http normalized to https",
                input: "http://www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "apex domain watch",
                input: "https://youtube.com/watch?v=dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "mobile m.youtube.com watch",
                input: "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "uppercase host accepted and normalized",
                input: "HTTPS://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "youtu.be short link",
                input: "https://youtu.be/dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "youtu.be http normalized",
                input: "http://youtu.be/dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "youtu.be with trailing slash",
                input: "https://youtu.be/dQw4w9WgXcQ/",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "youtu.be with si tracking query",
                input: "https://youtu.be/dQw4w9WgXcQ?si=AbCdEfGhIjK",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "live path",
                input: "https://www.youtube.com/live/dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "live path with feature query",
                input: "https://www.youtube.com/live/dQw4w9WgXcQ?feature=share",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "shorts path",
                input: "https://www.youtube.com/shorts/dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "mobile shorts path",
                input: "https://m.youtube.com/shorts/dQw4w9WgXcQ/",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "embed path",
                input: "https://www.youtube.com/embed/dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "id containing underscore and dash",
                input: "https://www.youtube.com/watch?v=A_b-C12_9-Z",
                expected_id: "A_b-C12_9-Z",
                expected_start: None,
            },
            ValidCase {
                name: "watch with both v and list treats as single video",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL1234567890&index=3",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "list before v still extracts single video",
                input: "https://www.youtube.com/watch?list=PLabc&v=dQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "youtu.be with list ignored as single video",
                input: "https://youtu.be/dQw4w9WgXcQ?list=RDdQw4w9WgXcQ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "t numeric seconds",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=95",
                expected_id: SAMPLE_ID,
                expected_start: Some(95),
            },
            ValidCase {
                name: "t with s suffix",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
                expected_id: SAMPLE_ID,
                expected_start: Some(42),
            },
            ValidCase {
                name: "t minute second notation",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=2m15s",
                expected_id: SAMPLE_ID,
                expected_start: Some(135),
            },
            ValidCase {
                name: "t hour minute second notation",
                input: "https://youtu.be/dQw4w9WgXcQ?t=1h2m3s",
                expected_id: SAMPLE_ID,
                expected_start: Some(3723),
            },
            ValidCase {
                name: "start param on embed",
                input: "https://www.youtube.com/embed/dQw4w9WgXcQ?start=120",
                expected_id: SAMPLE_ID,
                expected_start: Some(120),
            },
            ValidCase {
                name: "t preferred over start",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=30&start=60",
                expected_id: SAMPLE_ID,
                expected_start: Some(30),
            },
            ValidCase {
                name: "oversized t parameter capped cleanly",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=99999999999999",
                expected_id: SAMPLE_ID,
                expected_start: Some(MAX_SUGGESTED_START_SECONDS),
            },
            ValidCase {
                name: "zero t parameter",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=0",
                expected_id: SAMPLE_ID,
                expected_start: Some(0),
            },
            ValidCase {
                name: "malformed t ignored cleanly",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=-5",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "outer ASCII space padding trimmed",
                input: "  https://www.youtube.com/watch?v=dQw4w9WgXcQ  ",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
            ValidCase {
                name: "fragment stripped in canonical url",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ#t=10s",
                expected_id: SAMPLE_ID,
                expected_start: None,
            },
        ];

        assert!(cases.len() >= 25);
        for case in cases {
            let parsed = parse_youtube_url(case.input)
                .unwrap_or_else(|e| panic!("Case '{}' unexpectedly failed: {:?}", case.name, e));
            assert_eq!(parsed.video_id, case.expected_id, "case '{}'", case.name);
            assert_eq!(
                parsed.canonical_url,
                format!("https://www.youtube.com/watch?v={}", case.expected_id),
                "case '{}'",
                case.name
            );
            if case.expected_id == SAMPLE_ID {
                assert_eq!(parsed.canonical_url, EXPECTED_CANONICAL);
            }
            assert_eq!(
                parsed.suggested_start_seconds, case.expected_start,
                "case '{}'",
                case.name
            );
        }
    }

    #[test]
    fn test_invalid_youtube_urls_and_attacks_table_driven() {
        let too_long_url = format!(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&pad={}",
            "a".repeat(2020)
        );
        assert!(too_long_url.len() > MAX_YOUTUBE_URL_LENGTH);

        struct InvalidCase<'a> {
            name: &'a str,
            input: &'a str,
            expected_reason: UrlInvalidReason,
        }

        let cases = [
            InvalidCase {
                name: "empty string",
                input: "",
                expected_reason: UrlInvalidReason::Empty,
            },
            InvalidCase {
                name: "spaces only",
                input: "     ",
                expected_reason: UrlInvalidReason::Empty,
            },
            InvalidCase {
                name: "too long url",
                input: &too_long_url,
                expected_reason: UrlInvalidReason::TooLong,
            },
            InvalidCase {
                name: "missing scheme",
                input: "www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::InvalidSyntax,
            },
            InvalidCase {
                name: "javascript scheme",
                input: "javascript:alert(1)//youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::InvalidSyntax,
            },
            InvalidCase {
                name: "file scheme",
                input: "file://www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UnsupportedScheme,
            },
            InvalidCase {
                name: "ftp scheme",
                input: "ftp://www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UnsupportedScheme,
            },
            InvalidCase {
                name: "userinfo attack user@host",
                input: "https://youtube.com@evil.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UserinfoNotAllowed,
            },
            InvalidCase {
                name: "empty userinfo @host",
                input: "https://@www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UserinfoNotAllowed,
            },
            InvalidCase {
                name: "userinfo user:pass@youtube.com",
                input: "https://user:secret@www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UserinfoNotAllowed,
            },
            InvalidCase {
                name: "explicit extra port 8080",
                input: "https://www.youtube.com:8080/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::PortNotAllowed,
            },
            InvalidCase {
                name: "explicit default port 443",
                input: "https://www.youtube.com:443/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::PortNotAllowed,
            },
            InvalidCase {
                name: "punycode host",
                input: "https://xn--youtub-e2a.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "cyrillic homoglyph o in youtube.com",
                input: "https://www.y\u{043E}utube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "non-youtube subdomain evil",
                input: "https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "non-youtube hyphen domain",
                input: "https://evil-youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "unlisted subdomain music.youtube.com",
                input: "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "trailing dot on fqdn",
                input: "https://www.youtube.com./watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "vimeo url rejected",
                input: "https://vimeo.com/12345678901",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "localhost ssrf rejected",
                input: "http://127.0.0.1/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::HostNotAllowed,
            },
            InvalidCase {
                name: "ipv6 literal rejected",
                input: "http://[::1]/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::PortNotAllowed,
            },
            InvalidCase {
                name: "internal space",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ --exec evil",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "internal tab",
                input: "https://www.youtube.com/watch?v=dQw4\tw9WgXcQ",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "newline control char",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ\n",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "null byte control char",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ\0",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "zero-width space inside",
                input: "https://www.youtube.com/watch?v=dQw4\u{200B}w9WgXcQ",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "zero-width non-joiner inside host",
                input: "https://www.you\u{200C}tube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "bidi override RLO attack",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ\u{202E}",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "BOM character",
                input: "\u{FEFF}https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::UnsafeCharacters,
            },
            InvalidCase {
                name: "backslash path confusion",
                input: "https://www.youtube.com\\watch?v=dQw4w9WgXcQ",
                expected_reason: UrlInvalidReason::InvalidSyntax,
            },
            InvalidCase {
                name: "pure playlist url",
                input: "https://www.youtube.com/playlist?list=PL590L5WQmH8fJ54F369BLDSqHz3ZrwGtF",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "watch with list only no v",
                input: "https://www.youtube.com/watch?list=PL590L5WQmH8fJ54F369BLDSqHz3ZrwGtF",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "channel @handle url",
                input: "https://www.youtube.com/@MrBeast",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "channel @handle videos tab",
                input: "https://www.youtube.com/@MrBeast/videos",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "channel id url",
                input: "https://www.youtube.com/channel/UCX6OQ3DkcsbYNE6H8uQQuVA",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "search results url",
                input: "https://www.youtube.com/results?search_query=podcast",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "legacy user url",
                input: "https://www.youtube.com/user/YouTube",
                expected_reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            },
            InvalidCase {
                name: "short video id 10 chars",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXc",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "long video id 12 chars",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQQ",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "empty v param",
                input: "https://www.youtube.com/watch?v=",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "duplicate v params",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXcQ&v=9bZkp7q19f0",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "double encoded percent25 in id",
                input: "https://www.youtube.com/watch?v=dQw4w9WgXc%2551",
                expected_reason: UrlInvalidReason::DoubleEncodedOrInvalidId,
            },
            InvalidCase {
                name: "single percent encoded id char",
                input: "https://www.youtube.com/watch?v=%64Qw4w9WgXcQ",
                expected_reason: UrlInvalidReason::DoubleEncodedOrInvalidId,
            },
            InvalidCase {
                name: "double encoded shorts path",
                input: "https://www.youtube.com/shorts/%2564Qw4w9WgXcQ",
                expected_reason: UrlInvalidReason::DoubleEncodedOrInvalidId,
            },
            InvalidCase {
                name: "percent encoded id on youtu.be",
                input: "https://youtu.be/%64Qw4w9WgXcQ",
                expected_reason: UrlInvalidReason::DoubleEncodedOrInvalidId,
            },
            InvalidCase {
                name: "youtu.be extra path segment",
                input: "https://youtu.be/dQw4w9WgXcQ/extra",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "youtu.be root without id",
                input: "https://youtu.be/",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "shorts extra segment",
                input: "https://www.youtube.com/shorts/dQw4w9WgXcQ/remix",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "root homepage youtube.com",
                input: "https://www.youtube.com/",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "argument injection starting with dash in path",
                input: "https://www.youtube.com/watch?v=--help",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
            InvalidCase {
                name: "11-char dash flag argument injection rejected or safe",
                input: "https://www.youtube.com/watch?v=dQw4w9$XcQ1",
                expected_reason: UrlInvalidReason::MissingOrInvalidVideoId,
            },
        ];

        assert!(cases.len() >= 45);
        for case in cases {
            let err = parse_youtube_url(case.input).expect_err(&format!(
                "Attack/invalid case '{}' unexpectedly succeeded!",
                case.name
            ));
            assert_eq!(err.code(), "E_YT_URL_INVALID", "case '{}'", case.name);
            assert_eq!(
                err.reason(),
                case.expected_reason,
                "case '{}' wrong reason",
                case.name
            );
        }
    }
}
