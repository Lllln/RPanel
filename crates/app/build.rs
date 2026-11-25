fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Match macOS / Zed's 8 MB main thread stack on Windows MSVC.
    // The default 1 MB Win32 stack can overflow inside WndProc during deep GPUI + Tree-sitter
    // editor updates (especially in debug builds or with VDI/IME hook DLLs loaded).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
}
