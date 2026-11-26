//! Test harness executing production supervisor code (`spawn_streaming`)
//! for the abrupt termination / orphan-process verification test.

use clipper_helper_desktop::sidecars::{create_component_spec, SidecarSupervisor};
use std::io::Write;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: harness-supervisor <path_to_fake_sidecar>");
        std::process::exit(1);
    }
    let fake_sidecar = PathBuf::from(&args[1]);
    let ws_root = std::env::temp_dir().join(format!("clipper-harness-{}", std::process::id()));

    let supervisor = SidecarSupervisor::new();
    let spec = match create_component_spec(
        "fake-sidecar",
        &fake_sidecar,
        ["--mode", "tree-child"],
        &ws_root,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to prepare SidecarEnv: {}", e);
            std::process::exit(1);
        }
    };

    // Stream lines and forward them to our own stdout so test can read child and grandchild PIDs
    let res = supervisor
        .spawn_streaming(
            &spec,
            |line| {
                println!("{}", line);
                let _ = std::io::stdout().flush();
            },
            None,
        )
        .await;

    match res {
        Ok(r) => std::process::exit(r.exit_code),
        Err(_) => std::process::exit(1),
    }
}
