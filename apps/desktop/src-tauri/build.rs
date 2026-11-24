fn main() {
    println!("cargo:rerun-if-env-changed=CLIPPER_MIN_MANIFEST_SEQUENCE");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let is_release = profile == "release";

    let raw_seq = std::env::var("CLIPPER_MIN_MANIFEST_SEQUENCE").ok();
    let seq: u64 = match raw_seq {
        Some(val) if !val.trim().is_empty() => match val.trim().parse::<u64>() {
            Ok(n) if n >= 1 => n,
            Ok(n) => panic!("CLIPPER_MIN_MANIFEST_SEQUENCE must be >= 1, got {}", n),
            Err(e) => panic!(
                "CLIPPER_MIN_MANIFEST_SEQUENCE must be a positive integer (got '{}'): {}",
                val, e
            ),
        },
        _ => {
            if is_release {
                panic!(
                    "Release builds require CLIPPER_MIN_MANIFEST_SEQUENCE environment variable to be set (>= 1)"
                );
            } else {
                1
            }
        }
    };

    println!("cargo:rustc-env=CLIPPER_MIN_MANIFEST_SEQUENCE={}", seq);
    tauri_build::build();
}
