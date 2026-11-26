use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    if target_os == "windows" {
        // Embed the multi-resolution Windows application & taskbar icon (`assets/icon.ico`)
        // as resource ID 1, which both Windows Explorer and GPUI's `load_icon()` pick up.
        let rc_file = Path::new("resources/rpanel.rc");
        let assets_dir = Path::new("../../assets");
        let ico_file = assets_dir.join("icon.ico");
        println!("cargo:rerun-if-changed={}", rc_file.display());
        println!("cargo:rerun-if-changed={}", ico_file.display());
        embed_resource::compile(rc_file, embed_resource::ParamsIncludeDirs([assets_dir]))
            .manifest_optional()
            .unwrap();

        // Match macOS / Zed's 8 MB main thread stack on Windows MSVC.
        // The default 1 MB Win32 stack can overflow inside WndProc during deep GPUI
        // editor updates (especially in debug builds or with VDI/IME hook DLLs loaded).
        if target_env == "msvc" {
            println!("cargo:rustc-link-arg=/STACK:8388608");
        }
    }
}
