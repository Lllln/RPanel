#[cfg(any(windows, target_os = "macos"))]
mod app;
#[cfg(any(windows, target_os = "macos"))]
mod platform;
#[cfg(any(windows, target_os = "macos"))]
mod ui;

#[cfg(any(windows, target_os = "macos"))]
fn main() -> std::process::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("rpanel=info"))
        .init();

    if let Err(error) = app::run() {
        log::error!("RPanel 启动失败：{error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("RPanel 的交互原型支持 Windows 和 macOS。可运行 cargo test --lib 验证手势判定。");
}
