use std::thread;

use crossbeam_channel::Sender;
use tracing::warn;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Commands emitted from the system tray menu into the GPUI main event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenFromClipboard,
    ToggleEnabled,
    SetHoldMs(u64),
    Quit,
}

const THRESHOLD_PRESETS_MS: [u64; 4] = [250, 300, 450, 600];

/// Owns the native tray icon and menu check items for the lifetime of the app.
pub struct TrayController {
    _tray: TrayIcon,
    enabled_item: CheckMenuItem,
    threshold_items: Vec<(u64, CheckMenuItem)>,
}

impl TrayController {
    pub fn init(
        initial_hold_ms: u64,
        command_tx: Sender<TrayCommand>,
    ) -> Result<Self, String> {
        let open_item = MenuItem::new("打开 JSON 编辑器（剪贴板）", true, None);
        let enabled_item = CheckMenuItem::new("启用右键长按拦截", true, true, None);

        let threshold_menu = Submenu::new("长按时长阈值", true);
        let mut threshold_items = Vec::with_capacity(THRESHOLD_PRESETS_MS.len());
        let mut threshold_ids = Vec::with_capacity(THRESHOLD_PRESETS_MS.len());

        for ms in THRESHOLD_PRESETS_MS {
            let label = if ms == 300 {
                format!("{ms} ms（默认）")
            } else {
                format!("{ms} ms")
            };
            let item = CheckMenuItem::new(label, true, ms == initial_hold_ms, None);
            threshold_menu
                .append(&item)
                .map_err(|e| format!("failed to append threshold menu item: {e}"))?;
            threshold_ids.push((item.id().clone(), ms));
            threshold_items.push((ms, item));
        }

        let quit_item = MenuItem::new("退出 RPanel", true, None);

        let menu = Menu::new();
        menu.append(&open_item)
            .map_err(|e| format!("failed to append open menu item: {e}"))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("failed to append separator: {e}"))?;
        menu.append(&enabled_item)
            .map_err(|e| format!("failed to append enabled item: {e}"))?;
        menu.append(&threshold_menu)
            .map_err(|e| format!("failed to append threshold submenu: {e}"))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("failed to append separator: {e}"))?;
        menu.append(&quit_item)
            .map_err(|e| format!("failed to append quit item: {e}"))?;

        let icon = build_tray_icon()?;
        let builder = TrayIconBuilder::new()
            .with_tooltip("RPanel — Right-Click Floating Utility Panel")
            .with_menu(Box::new(menu));
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon);
        let tray = builder
            .build()
            .map_err(|e| format!("failed to build tray icon: {e}"))?;

        let open_id: MenuId = open_item.id().clone();
        let enabled_id: MenuId = enabled_item.id().clone();
        let quit_id: MenuId = quit_item.id().clone();

        thread::Builder::new()
            .name("rpanel-tray-events".into())
            .spawn(move || {
                let rx = MenuEvent::receiver();
                while let Ok(event) = rx.recv() {
                    let cmd = if event.id == open_id {
                        Some(TrayCommand::OpenFromClipboard)
                    } else if event.id == enabled_id {
                        Some(TrayCommand::ToggleEnabled)
                    } else if event.id == quit_id {
                        Some(TrayCommand::Quit)
                    } else {
                        threshold_ids
                            .iter()
                            .find(|(id, _)| *id == event.id)
                            .map(|(_, ms)| TrayCommand::SetHoldMs(*ms))
                    };

                    if let Some(cmd) = cmd
                        && command_tx.send(cmd).is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| format!("failed to spawn tray event listener: {e}"))?;

        Ok(Self {
            _tray: tray,
            enabled_item,
            threshold_items,
        })
    }

    pub fn sync_enabled(&self, enabled: bool) {
        self.enabled_item.set_checked(enabled);
    }

    pub fn sync_threshold(&self, active_ms: u64) {
        for (ms, item) in &self.threshold_items {
            item.set_checked(*ms == active_ms);
        }
    }
}

/// Load the pre-rendered 32x32 RGBA RPanel B2 tray icon (monochrome template on macOS,
/// full-color silver-titanium + Apple blue badge on Windows).
fn build_tray_icon() -> Result<Icon, String> {
    const SIZE: u32 = 32;
    #[cfg(target_os = "macos")]
    const RAW_RGBA: &[u8] = include_bytes!("../../../assets/tray-macos-32.rgba");
    #[cfg(not(target_os = "macos"))]
    const RAW_RGBA: &[u8] = include_bytes!("../../../assets/tray-windows-32.rgba");

    Icon::from_rgba(RAW_RGBA.to_vec(), SIZE, SIZE).map_err(|e| {
        warn!("failed to construct RGBA tray icon: {e}");
        e.to_string()
    })
}
