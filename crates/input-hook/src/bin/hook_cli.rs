use std::env;
use std::time::Duration;

use input_hook::{HookConfig, HookError, HookEvent, start_hook};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("input_hook=debug")),
        )
        .init();

    let mut config = HookConfig::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hold-ms" => {
                if let Some(val) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    config.hold_duration = Duration::from_millis(val.max(100));
                }
            }
            "--move-px" => {
                if let Some(val) = args.next().and_then(|v| v.parse::<i32>().ok()) {
                    config.max_move_px = val.max(1);
                }
            }
            "-h" | "--help" => {
                println!(
                    "RPanel input-hook CLI\n\nUsage:\n  hook-cli [--hold-ms 300] [--move-px 8]\n\nTips:\n  - Short right-click: should open native OS context menu normally.\n  - Right-drag: should drag normally.\n  - Hold right button >= hold-ms within move-px: swallows menu & captures selection.\n  - Press Ctrl+C in terminal to exit."
                );
                return;
            }
            other => {
                eprintln!("Unknown argument: {other} (use --help for usage)");
                std::process::exit(2);
            }
        }
    }

    println!(
        "Starting RPanel hook-cli (hold_ms={}, max_move_px={})...",
        config.hold_duration.as_millis(),
        config.max_move_px
    );

    let handle = match start_hook(config) {
        Ok(h) => h,
        Err(HookError::AccessibilityPermissionRequired) => {
            eprintln!(
                "macOS Accessibility permission is required.\nGrant permission in: System Settings -> Privacy & Security -> Accessibility, then re-run."
            );
            #[cfg(target_os = "macos")]
            input_hook::platform::macos::permission::open_accessibility_settings();
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("Failed to start global input hook: {err}");
            std::process::exit(1);
        }
    };

    println!("Hook active. Select text anywhere and hold right mouse button. Press Ctrl+C to quit.");

    for event in &handle.events {
        match event {
            HookEvent::LongPressTriggered { position, captured } => {
                println!(
                    "\n[LONG PRESS] pos=({}, {}) source={:?}",
                    position.x, position.y, captured.source
                );
                match captured.text {
                    Some(text) => println!("--- Captured Text ({} chars) ---\n{}\n---", text.chars().count(), text),
                    None => println!("(no selection captured)"),
                }
            }
            HookEvent::GlobalMouseDown { .. } => {}
            HookEvent::PermissionRequired => {
                eprintln!("[WARN] OS accessibility permission is required.");
            }
        }
    }

    handle.stop();
}
