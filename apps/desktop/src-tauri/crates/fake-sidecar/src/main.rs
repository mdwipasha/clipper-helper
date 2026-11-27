//! Fake sidecar binary for integration testing the sidecar supervisor and M1.4 download pipeline.
//!
//! Usage: fake-sidecar --mode <mode> [args...]
//! OR role-dispatched via production yt-dlp/ffmpeg/ffprobe CLI flags when
//! `CLIPPER_FAKE_SCENARIO` is configured in the child environment.

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn handle_ytdlp_probe(args: &[String]) {
    let scenario = env::var("CLIPPER_FAKE_PROBE_SCENARIO").unwrap_or_else(|_| "fresh".to_string());
    let video_id = args
        .last()
        .and_then(|u| u.rsplit('=').next())
        .filter(|s| s.len() == 11)
        .unwrap_or("WufkeDaJisQ");

    if scenario == "fail" {
        eprintln!(
            "ERROR: [youtube] {}: HTTP Error 429: Too Many Requests",
            video_id
        );
        std::process::exit(1);
    }

    let payload = format!(
        r#"{{"id":"{}","title":"Fake VOD Title","uploader":"Fake Channel","duration":600,"live_status":"not_live","availability":"public","playable_in_embed":true,"requested_formats":[{{"vcodec":"avc1.64002A","acodec":"none","height":1080,"tbr":3400,"url":"https://example.invalid/video?expire=9999999999"}},{{"vcodec":"none","acodec":"mp4a.40.2","tbr":128,"url":"https://example.invalid/audio?expire=9999999999"}}]}}"#,
        video_id
    );
    println!("{}", payload);
}

fn write_dummy_media(out_arg: &str) {
    let path = PathBuf::from(out_arg.replace("%(ext)s", "mp4"));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, b"FAKE_MP4_HEADER_AND_STREAM_PAYLOAD");
}

fn handle_ytdlp_download(args: &[String]) {
    let scenario =
        env::var("CLIPPER_FAKE_YTDLP_SCENARIO").unwrap_or_else(|_| "success".to_string());
    let out_arg = arg_value(args, "-o").unwrap_or_else(|| "out.mp4".to_string());

    match scenario.as_str() {
        "success" => {
            let mut err = std::io::stderr().lock();
            for sec in [5, 15, 30, 45, 60] {
                let _ = write!(
                    err,
                    "\rframe=  {} fps= 90 size= {}KiB time=00:00:{:02}.00 bitrate=1000.0kbits/s speed=2.00x",
                    sec * 30,
                    sec * 128,
                    sec
                );
                let _ = err.flush();
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            let _ = writeln!(err);
            write_dummy_media(&out_arg);
        }
        "403_then_success" => {
            let marker = env::var("CLIPPER_FAKE_403_MARKER")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(&out_arg).with_extension("403marker"));
            if !marker.exists() {
                if let Some(p) = marker.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let _ = std::fs::write(&marker, b"1");
                eprintln!("ERROR: unable to download video data: HTTP Error 403: Forbidden");
                std::process::exit(1);
            }
            write_dummy_media(&out_arg);
        }
        "429" => {
            eprintln!("ERROR: unable to download video data: HTTP Error 429: Too Many Requests");
            std::process::exit(1);
        }
        "bot_check" => {
            eprintln!("ERROR: [youtube] WufkeDaJisQ: Sign in to confirm you're not a bot.");
            std::process::exit(1);
        }
        "disconnect" => {
            eprintln!("frame= 100 fps= 60 size= 256KiB time=00:00:03.00 bitrate= 800.0kbits/s speed=1.50x");
            eprintln!("ERROR: [download] Got error: Connection reset by peer (errno 10054)");
            std::process::exit(1);
        }
        "flood" => {
            let mut err = std::io::stderr().lock();
            let line = "FLOOD_LINE_0123456789_".repeat(160);
            loop {
                if writeln!(err, "{}", line).is_err() {
                    break;
                }
            }
        }
        "stall" => {
            eprintln!("frame= 50 fps= 30 size= 100KiB time=00:00:02.00 speed=1.00x");
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        "invalid_utf8" => {
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(&[0xFF, 0xFE, 0x80, b'\n']);
            let _ = writeln!(
                err,
                "frame= 900 fps= 90 size= 2048KiB time=00:00:30.00 bitrate=1000.0kbits/s speed=1.80x"
            );
            let _ = err.flush();
            write_dummy_media(&out_arg);
        }
        "slow_start" => {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(
                err,
                "frame= 0 fps=0.0 q=0.0 size= 0KiB time=N/A bitrate=N/A speed=N/A"
            );
            std::thread::sleep(std::time::Duration::from_millis(80));
            let _ = writeln!(
                err,
                "frame= 1800 fps= 90 size= 4096KiB time=00:01:00.00 bitrate=1200.0kbits/s speed=1.66x"
            );
            write_dummy_media(&out_arg);
        }
        "tree_child" => {
            let exe = env::current_exe().expect("get current exe");
            let grandchild = Command::new(&exe)
                .args(["--mode", "sleep", "30000"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn grandchild");
            println!(
                "TREE_READY child={} grandchild={}",
                std::process::id(),
                grandchild.id()
            );
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        other => {
            eprintln!("Unknown fake yt-dlp scenario: {}", other);
            std::process::exit(2);
        }
    }
}

fn handle_ffprobe(args: &[String]) {
    let scenario = env::var("CLIPPER_FAKE_FFPROBE_SCENARIO").unwrap_or_else(|_| "avc1".to_string());
    let dur = env::var("CLIPPER_FAKE_FFPROBE_DURATION").unwrap_or_else(|_| "70.0".to_string());
    let target = args.last().map(String::as_str).unwrap_or("");
    let avc1_json = format!(
        r#"{{"streams":[{{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"60/1","r_frame_rate":"60/1"}},{{"codec_type":"audio","codec_name":"aac","sample_rate":"48000"}}],"format":{{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"{}","size":"7340032"}}}}"#,
        dur
    );
    let vp9_json = format!(
        r#"{{"streams":[{{"codec_type":"video","codec_name":"vp9","width":1920,"height":1080,"avg_frame_rate":"60/1","r_frame_rate":"60/1"}},{{"codec_type":"audio","codec_name":"opus","sample_rate":"48000"}}],"format":{{"format_name":"matroska,webm","duration":"{}","size":"5242880"}}}}"#,
        dur
    );
    let json = match scenario.as_str() {
        "avc1" => avc1_json,
        "vp9" => vp9_json,
        "vp9_then_avc1" => {
            if target.contains("proxy_") {
                avc1_json
            } else {
                vp9_json
            }
        }
        "bad_duration" => r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30/1"},{"codec_type":"audio","codec_name":"aac"}],"format":{"format_name":"mp4","duration":"12.0","size":"102400"}}"#.to_string(),
        "no_audio" => format!(
            r#"{{"streams":[{{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30/1"}}],"format":{{"format_name":"mp4","duration":"{}","size":"102400"}}}}"#,
            dur
        ),
        "fail" => {
            eprintln!("Invalid data found when processing input");
            std::process::exit(1);
        }
        other => {
            eprintln!("Unknown fake ffprobe scenario: {}", other);
            std::process::exit(2);
        }
    };
    println!("{}", json);
}

fn handle_ffmpeg_proxy(args: &[String]) {
    let scenario =
        env::var("CLIPPER_FAKE_FFMPEG_SCENARIO").unwrap_or_else(|_| "success".to_string());
    if scenario == "fail" {
        eprintln!("[h264_mf] Error initializing MediaFoundation encoder");
        std::process::exit(1);
    }
    let out_path = args
        .last()
        .cloned()
        .unwrap_or_else(|| "proxy.mp4".to_string());
    let mut out = std::io::stdout().lock();
    for step in [20_000_000u64, 45_000_000, 70_000_000] {
        let _ = writeln!(out, "frame={}", (step / 1_000_000) * 30);
        let _ = writeln!(out, "fps=105.0");
        let _ = writeln!(out, "total_size=1048576");
        let _ = writeln!(out, "out_time_us={}", step);
        let _ = writeln!(out, "speed=3.50x");
        let _ = writeln!(out, "progress=continue");
        let _ = out.flush();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = writeln!(out, "progress=end");
    let _ = out.flush();
    write_dummy_media(&out_path);
}

fn handle_ffmpeg_thumbnails(args: &[String]) {
    let scenario =
        env::var("CLIPPER_FAKE_THUMBS_SCENARIO").unwrap_or_else(|_| "success".to_string());
    if scenario == "fail" {
        eprintln!("Error opening thumbnail output");
        std::process::exit(1);
    }
    let pattern = args.last().cloned().unwrap_or_default();
    let parent = PathBuf::from(&pattern)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&parent);

    let mut out = std::io::stdout().lock();
    for (idx, us) in [2_000_000u64, 4_000_000, 6_000_000, 8_000_000, 10_000_000]
        .iter()
        .enumerate()
    {
        let _ = writeln!(out, "out_time_us={}", us);
        let _ = writeln!(out, "progress=continue");
        let _ = out.flush();
        let file = parent.join(format!("thumb_{:04}.jpg", idx + 1));
        let _ = std::fs::write(file, b"\xFF\xD8\xFF\xE0FAKE_JPEG_FRAME");
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    let _ = writeln!(out, "progress=end");
    let _ = out.flush();
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Production-flag dispatch for M1.4/M1.5 tests using real CLI argument arrays:
    if args.iter().any(|a| a == "-J") {
        handle_ytdlp_probe(&args);
        return;
    }
    if args.iter().any(|a| a == "--download-sections") {
        handle_ytdlp_download(&args);
        return;
    }
    if args.iter().any(|a| a == "-show_streams") {
        handle_ffprobe(&args);
        return;
    }
    if args.iter().any(|a| a.contains("thumb_%04d.jpg")) {
        handle_ffmpeg_thumbnails(&args);
        return;
    }
    if args.iter().any(|a| a == "pipe:1") {
        handle_ffmpeg_proxy(&args);
        return;
    }

    if args.len() < 3 || args[1] != "--mode" {
        eprintln!("Usage: fake-sidecar --mode <mode> [args...]");
        std::process::exit(2);
    }

    let mode = &args[2];
    match mode.as_str() {
        "normal" => {
            println!("hello");
        }
        "exit-code" => {
            let code: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            std::process::exit(code);
        }
        "progress" => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for i in 0..=10 {
                // \r-terminated progress lines (like FFmpeg)
                write!(out, "\rProgress: {}%", i * 10).ok();
                out.flush().ok();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            writeln!(out).ok(); // Final newline
            println!("done");
        }
        "sleep" => {
            let ms: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1000);
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        "flood" => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let line = "X".repeat(4096);
            loop {
                if writeln!(out, "{}", line).is_err() {
                    break; // Broken pipe — reader closed
                }
            }
        }
        "invalid-utf8" => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            // Write bytes that are not valid UTF-8
            let bad_bytes: &[u8] = &[0xFF, 0xFE, 0x80, 0x81, b'\n'];
            out.write_all(bad_bytes).ok();
            out.flush().ok();
        }
        "grandchild" => {
            let exe = env::current_exe().expect("get current exe");
            let child = Command::new(&exe)
                .args(["--mode", "sleep", "30000"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn grandchild");
            println!("grandchild pid: {}", child.id());
        }
        "env" => {
            for (key, val) in env::vars() {
                println!("{}={}", key, val);
            }
        }
        "pwd" => {
            if let Ok(current_dir) = env::current_dir() {
                println!("{}", current_dir.display());
            }
        }
        "tree-child" => {
            let exe = env::current_exe().expect("get current exe");
            let grandchild = Command::new(&exe)
                .args(["--mode", "sleep", "30000"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn grandchild");
            let child_pid = std::process::id();
            let grandchild_pid = grandchild.id();
            println!(
                "TREE_READY child={} grandchild={}",
                child_pid, grandchild_pid
            );
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_millis(30000));
        }
        other => {
            eprintln!("Unknown mode: {}", other);
            std::process::exit(2);
        }
    }
}
