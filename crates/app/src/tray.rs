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
        let open_item = MenuItem::new("Open JSON Panel (Clipboard)", true, None);
        let enabled_item = CheckMenuItem::new("Enable Right-Click Hold", true, true, None);

        let threshold_menu = Submenu::new("Long-Press Threshold", true);
        let mut threshold_items = Vec::with_capacity(THRESHOLD_PRESETS_MS.len());
        let mut threshold_ids = Vec::with_capacity(THRESHOLD_PRESETS_MS.len());

        for ms in THRESHOLD_PRESETS_MS {
            let label = if ms == 300 {
                format!("{ms} ms (Default)")
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

        let quit_item = MenuItem::new("Quit RPanel", true, None);

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
            .with_tooltip("RPanel — Right-Click Hold JSON Inspector")
            .with_menu(Box::new(menu));
        let builder = if cfg!(target_os = "macos") {
            builder.with_icon_templated(icon)
        } else {
            builder.with_icon(icon)
        };
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

/// Generate a crisp 32x32 RGBA rounded-square tray icon with `{ }` braces.
fn build_tray_icon() -> Result<Icon, String> {
    const SIZE: usize = 32;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let idx = (y * SIZE + x) * 4;

            // Signed distance to a centered rounded square (size 26x26, radius 6).
            let dx = (fx - 16.0).abs() - 7.0;
            let dy = (fy - 16.0).abs() - 7.0;
            let dist = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt()
                + dx.max(dy).min(0.0)
                - 6.0;

            if dist <= 0.6 {
                let alpha = ((0.6 - dist).clamp(0.0, 1.0) * 255.0) as u8;
                let in_glyph = is_brace_glyph(x, y);

                if cfg!(target_os = "macos") {
                    // Monochrome template icon: solid black border/glyph on transparent.
                    let is_border = dist > -1.8;
                    if is_border || in_glyph {
                        rgba[idx] = 0;
                        rgba[idx + 1] = 0;
                        rgba[idx + 2] = 0;
                        rgba[idx + 3] = alpha;
                    }
                } else {
                    // Windows tray icon: restrained dark slate tile with crisp white `{}` glyph.
                    if in_glyph {
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = 255;
                    } else {
                        rgba[idx] = 29;
                        rgba[idx + 1] = 29;
                        rgba[idx + 2] = 31;
                        rgba[idx + 3] = alpha;
                    }
                }
            }
        }
    }

    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).map_err(|e| {
        warn!("failed to construct RGBA tray icon: {e}");
        e.to_string()
    })
}

fn is_brace_glyph(x: usize, y: usize) -> bool {
    // Symmetric minimal `{ }` pixel glyph inside 32x32 tile.
    let left_brace = matches!(
        (x, y),
        (11..=13, 9..=10)
            | (10..=11, 11..=14)
            | (8..=10, 15..=16)
            | (10..=11, 17..=20)
            | (11..=13, 21..=22)
    );
    let right_brace = matches!(
        (x, y),
        (18..=20, 9..=10)
            | (20..=21, 11..=14)
            | (21..=23, 15..=16)
            | (20..=21, 17..=20)
            | (18..=20, 21..=22)
    );
    left_brace || right_brace
}
