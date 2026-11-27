//! Progress line parser ported from `spikes/s1-downloader/progress_parser.py`
//! plus FFmpeg `-progress pipe:1` accumulator (PROGRESS_AND_ETA §4.1; S1b).

use std::sync::LazyLock;

use regex::Regex;

static RE_FF_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"time=\s*([0-9:.]+|N/A|none|unknown)").expect("valid regex"));
static RE_FF_SPEED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"speed=\s*([0-9.]+[xX]|N/A|none)").expect("valid regex"));
static RE_FF_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"frame=\s*([0-9]+)").expect("valid regex"));
static RE_FF_FPS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"fps=\s*([0-9.]+)").expect("valid regex"));
static RE_FF_SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"size=\s*([0-9.]+\s*[A-Za-z]+)").expect("valid regex"));
static RE_FF_BITRATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"bitrate=\s*([0-9.]+)\s*kbits/s").expect("valid regex"));

static RE_YTDLP_PCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[download\]\s+([0-9.]+)%").expect("valid regex"));
static RE_YTDLP_ETA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"ETA\s+([0-9:]+)").expect("valid regex"));
static RE_YTDLP_SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"of\s+~?([0-9.]+\s*[A-Za-z]+)").expect("valid regex"));

static RE_SIZE_UNIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9.]+)\s*([A-Za-z]+)?$").expect("valid regex"));

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedProgressLine {
    pub time_s: Option<f64>,
    pub speed_ratio: Option<f64>,
    pub percent: Option<f64>,
    pub eta_s: Option<f64>,
    pub frame: Option<u64>,
    pub size_bytes: Option<u64>,
    pub bitrate_kbps: Option<f64>,
    pub fps: Option<f64>,
    pub raw_line: String,
}

fn round_n(val: f64, decimals: u32) -> f64 {
    let factor = 10f64.powi(decimals as i32);
    (val * factor).round() / factor
}

/// Parses `HH:MM:SS.ffffff`, `MM:SS.ffffff`, or seconds string into rounded seconds (3 decimals).
pub fn parse_time_str(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || matches!(trimmed, "N/A" | "none" | "unknown") {
        return None;
    }
    let parts: Vec<&str> = trimmed.split(':').collect();
    let val = match parts.len() {
        3 => {
            let h: f64 = parts[0].parse().ok()?;
            let m: f64 = parts[1].parse().ok()?;
            let s: f64 = parts[2].parse().ok()?;
            h * 3600.0 + m * 60.0 + s
        }
        2 => {
            let m: f64 = parts[0].parse().ok()?;
            let s: f64 = parts[1].parse().ok()?;
            m * 60.0 + s
        }
        1 => parts[0].parse::<f64>().ok()?,
        _ => return None,
    };
    if val.is_finite() && val >= 0.0 {
        Some(round_n(val, 3))
    } else {
        None
    }
}

/// Parses speed string like `'1.76x'` or `' 2.4X '` into positive multiplier.
pub fn parse_speed_str(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || matches!(trimmed, "N/A" | "none") {
        return None;
    }
    let stripped = trimmed.trim_end_matches(['x', 'X']).trim();
    let val: f64 = stripped.parse().ok()?;
    if val.is_finite() && val > 0.0 {
        Some(val)
    } else {
        None
    }
}

/// Parses size string like `'3584KiB'` or `'1.5MiB'` into byte count.
pub fn parse_size_str(raw: &str) -> Option<u64> {
    let caps = RE_SIZE_UNIT.captures(raw.trim())?;
    let val: f64 = caps.get(1)?.as_str().parse().ok()?;
    if !val.is_finite() || val < 0.0 {
        return None;
    }
    let unit = caps
        .get(2)
        .map(|m| m.as_str().to_ascii_lowercase())
        .unwrap_or_default();
    let mult: f64 = match unit.as_str() {
        "b" | "" => 1.0,
        "kib" => 1024.0,
        "kb" => 1000.0,
        "mib" => 1024.0 * 1024.0,
        "mb" => 1000.0 * 1000.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "gb" => 1000.0 * 1000.0 * 1000.0,
        _ => 1.0,
    };
    Some((val * mult) as u64)
}

/// Parses a single line (or `\r`-delimited chunk) from yt-dlp or FFmpeg stderr/stdout.
pub fn parse_progress_line(
    raw_chunk: &str,
    target_duration_s: Option<f64>,
) -> Option<ParsedProgressLine> {
    if raw_chunk.is_empty() {
        return None;
    }

    let line = if raw_chunk.contains('\r') {
        raw_chunk
            .split('\r')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .next_back()?
    } else {
        raw_chunk.trim()
    };

    if line.is_empty() {
        return None;
    }

    if line.contains("time=") {
        let mut update = ParsedProgressLine {
            raw_line: line.to_string(),
            ..Default::default()
        };

        if let Some(c) = RE_FF_TIME.captures(line) {
            update.time_s = parse_time_str(&c[1]);
        }
        if let Some(c) = RE_FF_SPEED.captures(line) {
            update.speed_ratio = parse_speed_str(&c[1]);
        }
        if let Some(c) = RE_FF_FRAME.captures(line) {
            update.frame = c[1].parse::<u64>().ok();
        }
        if let Some(c) = RE_FF_FPS.captures(line) {
            update.fps = c[1].parse::<f64>().ok();
        }
        if let Some(c) = RE_FF_SIZE.captures(line) {
            update.size_bytes = parse_size_str(&c[1]);
        }
        if let Some(c) = RE_FF_BITRATE.captures(line) {
            update.bitrate_kbps = c[1].parse::<f64>().ok();
        }

        if let (Some(time_s), Some(target)) = (update.time_s, target_duration_s) {
            if target > 0.0 {
                let pct = ((time_s / target) * 100.0).clamp(0.0, 100.0);
                update.percent = Some(round_n(pct, 1));
                if let Some(speed) = update.speed_ratio {
                    if speed > 0.0 && time_s < target {
                        let rem = ((target - time_s) / speed).max(0.0);
                        update.eta_s = Some(round_n(rem, 1));
                    }
                }
            }
        }
        return Some(update);
    }

    if line.contains("[download]") && line.contains('%') {
        let mut update = ParsedProgressLine {
            raw_line: line.to_string(),
            ..Default::default()
        };
        if let Some(c) = RE_YTDLP_PCT.captures(line) {
            update.percent = c[1].parse::<f64>().ok();
        }
        if let Some(c) = RE_YTDLP_ETA.captures(line) {
            update.eta_s = parse_time_str(&c[1]);
        }
        if let Some(c) = RE_YTDLP_SIZE.captures(line) {
            update.size_bytes = parse_size_str(&c[1]);
        }
        return Some(update);
    }

    None
}

/// Stateful parser for FFmpeg `-progress pipe:1` (`key=value` blocks ending in `progress=continue|end`).
#[derive(Debug, Default)]
pub struct FfmpegPipeProgressParser {
    target_duration_s: Option<f64>,
    current: ParsedProgressLine,
}

impl FfmpegPipeProgressParser {
    pub fn new(target_duration_s: Option<f64>) -> Self {
        Self {
            target_duration_s,
            current: ParsedProgressLine::default(),
        }
    }

    /// Feeds one line from FFmpeg `-progress pipe:1`. Returns `Some(...)` when a block finishes.
    pub fn push_line(&mut self, raw_line: &str) -> Option<ParsedProgressLine> {
        let line = raw_line.trim();
        let (key, val) = line.split_once('=')?;
        let key = key.trim();
        let val = val.trim();

        match key {
            "frame" => self.current.frame = val.parse::<u64>().ok(),
            "fps" => self.current.fps = val.parse::<f64>().ok(),
            "total_size" => self.current.size_bytes = val.parse::<u64>().ok(),
            "out_time_us" => {
                if let Ok(us) = val.parse::<f64>() {
                    if us >= 0.0 {
                        self.current.time_s = Some(round_n(us / 1_000_000.0, 3));
                    }
                }
            }
            "out_time_ms" => {
                // Note: ffmpeg `-progress` historically reports microseconds under `out_time_ms` too,
                // only populate if out_time_us was absent.
                if self.current.time_s.is_none() {
                    if let Ok(us) = val.parse::<f64>() {
                        if us >= 0.0 {
                            self.current.time_s = Some(round_n(us / 1_000_000.0, 3));
                        }
                    }
                }
            }
            "out_time" => {
                if self.current.time_s.is_none() {
                    self.current.time_s = parse_time_str(val);
                }
            }
            "speed" => self.current.speed_ratio = parse_speed_str(val),
            "progress" => {
                let mut completed = std::mem::take(&mut self.current);
                if val == "end" {
                    if let Some(t) = self.target_duration_s {
                        if completed.time_s.is_none() {
                            completed.time_s = Some(t);
                        }
                    }
                    completed.percent = Some(100.0);
                    completed.eta_s = Some(0.0);
                } else if let (Some(time_s), Some(target)) =
                    (completed.time_s, self.target_duration_s)
                {
                    if target > 0.0 {
                        completed.percent =
                            Some(round_n(((time_s / target) * 100.0).clamp(0.0, 100.0), 1));
                        if let Some(speed) = completed.speed_ratio {
                            if speed > 0.0 && time_s < target {
                                completed.eta_s = Some(round_n((target - time_s) / speed, 1));
                            }
                        }
                    }
                }
                completed.raw_line = line.to_string();
                return Some(completed);
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_ffmpeg_line() {
        let line = "frame= 1850 fps= 98.5 q=31.0 size= 3584KiB time=00:03:08.50 bitrate= 844.7kbits/s speed=1.76x";
        let res = parse_progress_line(line, Some(300.0)).unwrap();
        assert_eq!(res.frame, Some(1850));
        assert_eq!(res.fps, Some(98.5));
        assert_eq!(res.size_bytes, Some(3584 * 1024));
        assert_eq!(res.time_s, Some(188.5));
        assert_eq!(res.bitrate_kbps, Some(844.7));
        assert_eq!(res.speed_ratio, Some(1.76));
        assert_eq!(res.percent, Some(62.8));
        assert_eq!(res.eta_s, Some(63.4));
    }

    #[test]
    fn test_time_na_speed_na() {
        let line = "frame=    0 fps=0.0 q=0.0 size=       0KiB time=N/A bitrate=N/A speed=N/A";
        let res = parse_progress_line(line, Some(60.0)).unwrap();
        assert_eq!(res.frame, Some(0));
        assert_eq!(res.time_s, None);
        assert_eq!(res.speed_ratio, None);
        assert_eq!(res.percent, None);
        assert_eq!(res.eta_s, None);
    }

    #[test]
    fn test_carriage_return_multiline_chunk() {
        let chunk = "frame=  100 fps= 50 size= 500KiB time=00:00:10.00 speed=1.0x\rframe=  200 fps= 52 size= 1000KiB time=00:00:20.00 speed=1.2x";
        let res = parse_progress_line(chunk, Some(60.0)).unwrap();
        assert_eq!(res.frame, Some(200));
        assert_eq!(res.time_s, Some(20.0));
        assert_eq!(res.speed_ratio, Some(1.2));
        assert_eq!(res.percent, Some(33.3));
    }

    #[test]
    fn test_duration_over_10_hours() {
        let line = "frame=1234567 fps= 60 size= 123456KiB time=12:34:56.78 bitrate=1500.0kbits/s speed=2.50x";
        let res = parse_progress_line(line, Some(50000.0)).unwrap();
        assert_eq!(res.time_s, Some(45296.78));
        assert_eq!(res.speed_ratio, Some(2.50));
        assert_eq!(res.percent, Some(round_n((45296.78 / 50000.0) * 100.0, 1)));
    }

    #[test]
    fn test_ytdlp_native_download_line() {
        let line = "[download]  42.0% of ~118.00MiB at  6.20MiB/s ETA 00:12";
        let res = parse_progress_line(line, None).unwrap();
        assert_eq!(res.percent, Some(42.0));
        assert_eq!(res.eta_s, Some(12.0));
        assert_eq!(res.size_bytes, Some((118.0 * 1024.0 * 1024.0) as u64));
    }

    #[test]
    fn test_non_progress_lines() {
        let lines = [
            "[youtube] Extracting URL: https://www.youtube.com/watch?v=QOJIILaJKNc",
            "[youtube] QOJIILaJKNc: Downloading webpage",
            "[download] Destination: output.mp4",
            "",
            "   ",
            "\r\r\n",
        ];
        for line in lines {
            assert!(parse_progress_line(line, None).is_none());
        }
    }

    #[test]
    fn test_helpers() {
        assert_eq!(parse_time_str("00:01:05.500"), Some(65.5));
        assert_eq!(parse_time_str("01:05.500"), Some(65.5));
        assert_eq!(parse_time_str("N/A"), None);
        assert_eq!(parse_time_str(""), None);

        assert_eq!(parse_speed_str("1.5x"), Some(1.5));
        assert_eq!(parse_speed_str(" 2.4X "), Some(2.4));
        assert_eq!(parse_speed_str("N/A"), None);

        assert_eq!(parse_size_str("1024KiB"), Some(1024 * 1024));
        assert_eq!(parse_size_str("10MB"), Some(10 * 1000 * 1000));
        assert_eq!(
            parse_size_str("1.5MiB"),
            Some((1.5 * 1024.0 * 1024.0) as u64)
        );
    }

    #[test]
    fn test_ffmpeg_pipe_progress_parser() {
        let mut p = FfmpegPipeProgressParser::new(Some(60.0));
        assert!(p.push_line("frame=450").is_none());
        assert!(p.push_line("fps=90.0").is_none());
        assert!(p.push_line("total_size=5242880").is_none());
        assert!(p.push_line("out_time_us=15000000").is_none());
        assert!(p.push_line("speed=3.0x").is_none());
        let block = p.push_line("progress=continue").unwrap();
        assert_eq!(block.frame, Some(450));
        assert_eq!(block.fps, Some(90.0));
        assert_eq!(block.size_bytes, Some(5242880));
        assert_eq!(block.time_s, Some(15.0));
        assert_eq!(block.speed_ratio, Some(3.0));
        assert_eq!(block.percent, Some(25.0));
        assert_eq!(block.eta_s, Some(15.0));

        assert!(p.push_line("out_time_us=60000000").is_none());
        let end = p.push_line("progress=end").unwrap();
        assert_eq!(end.percent, Some(100.0));
        assert_eq!(end.eta_s, Some(0.0));
    }
}
