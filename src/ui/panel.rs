use std::{fs, path::PathBuf, sync::Arc};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Frame, RichText, Stroke};

pub const SIZE: [u32; 2] = [168, 52];

pub fn configure(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(29, 29, 31);
    visuals.widgets.inactive.bg_fill = Color32::WHITE;
    visuals.widgets.inactive.weak_bg_fill = Color32::WHITE;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(241, 243, 246);
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(232, 235, 241);
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    ctx.set_visuals(visuals);

    // 运行时加载各平台的系统中文字体，字体文件不随原型分发。
    if let Some(cjk) = system_cjk_font() {
        let mut fonts = FontDefinitions::default();
        let name = "system-cjk".to_owned();
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(cjk)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .push(name);
        ctx.set_fonts(fonts);
    } else {
        log::warn!("没有找到系统中文字体，原型菜单中文可能无法正常显示");
    }
}

fn system_cjk_font() -> Option<Vec<u8>> {
    #[cfg(windows)]
    let (font_dir, fonts) = (
        PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts"),
        ["msyh.ttc", "simsun.ttc", "msyh.ttf"],
    );
    #[cfg(target_os = "macos")]
    let (font_dir, fonts) = (
        PathBuf::from("/System/Library/Fonts"),
        [
            "PingFang.ttc",
            "Hiragino Sans GB.ttc",
            "Supplemental/Songti.ttc",
        ],
    );
    fonts
        .iter()
        .find_map(|name| fs::read(font_dir.join(name)).ok())
}

pub fn show(ui: &mut egui::Ui) -> bool {
    Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, Color32::from_rgb(221, 224, 229)))
        .corner_radius(12)
        .inner_margin(7)
        .show(ui, |ui| {
            let label = RichText::new("查看Json")
                .size(14.0)
                .color(Color32::from_rgb(29, 29, 31));
            ui.add(egui::Button::new(label).min_size(egui::vec2(ui.available_width(), 36.0)))
                .clicked()
        })
        .inner
}
