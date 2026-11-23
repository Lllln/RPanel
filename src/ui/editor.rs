use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Color32, Context, Frame, Stroke, ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::platform::Clipboard;

#[derive(Default)]
pub struct Editors {
    windows: Vec<EditorWindow>,
    next_id: u64,
}

struct EditorWindow {
    id: ViewportId,
    editor: Arc<Mutex<Option<Editor>>>,
}

impl Editors {
    pub fn open(&mut self, ctx: &Context, text: String, status: &'static str) {
        self.next_id = self.next_id.wrapping_add(1);
        self.windows.push(EditorWindow {
            id: ViewportId::from_hash_of(("json-editor", self.next_id)),
            editor: Arc::new(Mutex::new(Some(Editor {
                text,
                status,
                first_frame: true,
            }))),
        });
        refresh_root(ctx);
    }

    pub fn show(&mut self, ctx: &Context, clipboard: Clipboard) {
        self.windows
            .retain(|window| window.editor.lock().is_ok_and(|editor| editor.is_some()));
        for window in &self.windows {
            let editor = Arc::clone(&window.editor);
            ctx.show_viewport_deferred(
                window.id,
                ViewportBuilder::default()
                    .with_title("查看Json · RPanel")
                    .with_inner_size([720.0, 480.0])
                    .with_min_inner_size([360.0, 220.0])
                    .with_active(true),
                move |ui, _| {
                    let Ok(mut slot) = editor.lock() else {
                        return;
                    };
                    if ui.input(|input| input.viewport().close_requested()) {
                        *slot = None;
                        refresh_root(ui.ctx());
                    } else if let Some(editor) = slot.as_mut() {
                        editor.show(ui, clipboard);
                    }
                },
            );
        }
    }
}

fn refresh_root(ctx: &Context) {
    // 面板隐藏时 eframe 只运行 logic。创建或移除子窗口需要根 viewport 再绘制一帧。
    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::RequestPaintWhileHidden);
    ctx.request_repaint_of(ViewportId::ROOT);
}

struct Editor {
    text: String,
    status: &'static str,
    first_frame: bool,
}

impl Editor {
    fn show(&mut self, ui: &mut egui::Ui, clipboard: Clipboard) {
        let text_id = ui.scope_id().with("input");
        if std::mem::take(&mut self.first_frame) {
            ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
            ui.memory_mut(|memory| memory.request_focus(text_id));
        }

        let paste_button = egui::Panel::bottom("editor-footer")
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(246, 247, 249))
                    .inner_margin(10),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let paste = ui.button("粘贴").clicked();
                    ui.label(self.status);
                    paste
                })
                .inner
            })
            .inner;

        let paste_key =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
        if paste_button || paste_key {
            self.paste(ui, clipboard, text_id);
        }
        ui.input_mut(|input| {
            // 使用编辑器自己的选区和撤销逻辑，剪贴板写入由平台层反馈结果。
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::C) {
                input.events.push(egui::Event::Copy);
            }
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::X) {
                input.events.push(egui::Event::Cut);
            }
        });

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(246, 247, 249))
                    .inner_margin(10),
            )
            .show(ui, |ui| {
                Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, Color32::from_rgb(221, 224, 229)))
                    .corner_radius(4)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        let size = ui.available_size();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.text)
                                    .id(text_id)
                                    .code_editor()
                                    .frame(Frame::NONE)
                                    .min_size(size)
                                    .desired_width(f32::INFINITY)
                                    .text_color(Color32::from_rgb(29, 29, 31))
                                    .hint_text("粘贴或输入 JSON 文本"),
                            );
                        });
                    });
            });

        let mut copied = None;
        ui.output_mut(|output| {
            output.commands.retain_mut(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    copied = Some(std::mem::take(text));
                    false
                } else {
                    true
                }
            })
        });
        if let Some(text) = copied {
            let status = match clipboard.write(&text) {
                Ok(()) => "已复制到剪贴板。",
                Err(error) => {
                    log::warn!("编辑窗口写入剪贴板失败：{error}");
                    "无法写入剪贴板。"
                }
            };
            self.set_status(ui, status);
        }
    }

    fn paste(&mut self, ui: &egui::Ui, clipboard: Clipboard, id: egui::Id) {
        let status = match clipboard.read() {
            Ok(Some(text)) => {
                ui.memory_mut(|memory| memory.request_focus(id));
                ui.input_mut(|input| input.events.push(egui::Event::Paste(text)));
                "已从剪贴板粘贴文本。"
            }
            Ok(None) => "剪贴板中没有可粘贴的文本。",
            Err(error) => {
                log::warn!("编辑窗口读取剪贴板失败：{error}");
                "无法读取剪贴板，请重试。"
            }
        };
        self.set_status(ui, status);
    }

    fn set_status(&mut self, ui: &egui::Ui, status: &'static str) {
        self.status = status;
        ui.ctx().request_repaint();
    }
}
