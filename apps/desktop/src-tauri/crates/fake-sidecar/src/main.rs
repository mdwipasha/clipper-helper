//! Fake sidecar binary for integration testing the sidecar supervisor.
//!
//! Usage: fake-sidecar --mode <mode> [args...]
//!
//! Modes:
//!   normal          — Print "hello" to stdout and exit 0.
//!   exit-code <N>   — Exit with code N.
//!   progress        — Print \r-terminated progress lines with delays.
//!   sleep <ms>      — Sleep for ms milliseconds then exit 0.
//!   flood           — Continuously output data to stdout (for cap testing).
//!   invalid-utf8    — Write non-UTF-8 bytes to stdout.
//!   grandchild      — Spawn a child that sleeps, then exit immediately.

use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

fn main() {
    let args: Vec<String> = env::args().collect();
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
            // Spawn a child process that sleeps, then exit ourselves immediately.
            // The sidecar supervisor should kill the grandchild via Job Object.
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
            // Dump environment variables to stdout as KEY=VALUE
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
            // Spawn a grandchild that sleeps, then sleep ourselves.
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
