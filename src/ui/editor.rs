use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Color32, Context, Frame, Id, RichText, Stroke, Ui, ViewportBuilder, ViewportCommand,
    ViewportId,
};
use rpanel::core::json::{Diagnostic, Issue, Operation, Output, Report};

use super::{
    document::{Document, Selection},
    json_view,
    json_worker::{SubmitError, Worker},
};
use crate::platform::Clipboard;

const BACKGROUND: Color32 = Color32::from_rgb(246, 247, 249);

#[derive(Default)]
pub struct Editors {
    windows: Vec<EditorWindow>,
    next_id: u64,
    worker: Worker,
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
            editor: Arc::new(Mutex::new(Some(Editor::new(text, status)))),
        });
        refresh_root(ctx);
    }

    pub fn show(&mut self, ctx: &Context, clipboard: Clipboard) {
        self.windows
            .retain(|window| window.editor.lock().is_ok_and(|editor| editor.is_some()));
        for window in &self.windows {
            let editor = Arc::clone(&window.editor);
            let worker = self.worker.clone();
            ctx.show_viewport_deferred(
                window.id,
                ViewportBuilder::default()
                    .with_title("查看Json · RPanel")
                    .with_inner_size([720.0, 480.0])
                    .with_min_inner_size([380.0, 220.0])
                    .with_active(true),
                move |ui, _| {
                    let Ok(mut slot) = editor.lock() else {
                        return;
                    };
                    if ui.input(|input| input.viewport().close_requested()) {
                        *slot = None;
                        refresh_root(ui.ctx());
                    } else if let Some(editor) = slot.as_mut() {
                        editor.show(ui, clipboard, &worker);
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
    document: Document,
    status: &'static str,
    notice: Option<&'static str>,
    first_frame: bool,
    selection: Selection,
    history: History,
    serial: u64,
    scheduled: Option<Scheduled>,
    pending: Option<Pending>,
    report: Option<Report>,
}

struct Scheduled {
    operation: Operation,
    earliest: Instant,
    initial: bool,
}

struct Pending {
    serial: u64,
    operation: Operation,
    initial: bool,
    result: Receiver<Report>,
}

#[derive(Default)]
struct Toolbar {
    paste: bool,
    operation: Option<Operation>,
    jump: Option<usize>,
}

#[derive(Default)]
struct Shortcuts {
    paste: bool,
    copy: bool,
    cut: bool,
    history: Option<HistoryAction>,
}

impl Editor {
    fn new(text: String, status: &'static str) -> Self {
        let scheduled = (!text.is_empty()).then(|| Scheduled {
            operation: Operation::Format,
            earliest: Instant::now(),
            initial: true,
        });
        Self {
            document: Document::new(text),
            status,
            notice: None,
            first_frame: true,
            selection: Selection::default(),
            history: History::default(),
            serial: 0,
            scheduled,
            pending: None,
            report: None,
        }
    }

    fn show(&mut self, ui: &mut Ui, clipboard: Clipboard, worker: &Worker) {
        let text_id = ui.scope_id().with("input");
        if std::mem::take(&mut self.first_frame) {
            ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
            restore_selection(ui.ctx(), text_id, &self.document, self.selection);
            ui.memory_mut(|memory| memory.request_focus(text_id));
        }
        let toolbar = self.toolbar(ui);
        let shortcuts = shortcuts(ui);
        if let Some(action) = shortcuts.history {
            self.restore_history(ui.ctx(), text_id, action);
        }
        if toolbar.paste || shortcuts.paste {
            self.paste(ui, clipboard, text_id);
        }
        let jump = toolbar
            .jump
            .filter(|&byte| byte <= self.document.source.len() && self.report.is_some());
        if let Some(byte) = jump {
            self.document.reveal(byte);
            self.selection = Selection::caret(byte);
            restore_selection(ui.ctx(), text_id, &self.document, self.selection);
            ui.memory_mut(|memory| memory.request_focus(text_id));
        }

        let before = self.snapshot();
        let mut changed = self.draw_document(ui, text_id, jump);
        if (shortcuts.copy || shortcuts.cut) && !self.selection.sorted().is_empty() {
            changed |= self.copy_selection(ui, clipboard, text_id, shortcuts.cut);
        }
        if changed {
            self.history.record(before);
            self.document_changed();
            ui.ctx().request_repaint();
        }
        if let Some(operation) = toolbar.operation {
            self.schedule(operation, Duration::ZERO);
            self.notice = Some(if operation == Operation::Format {
                "正在格式化…"
            } else {
                "正在校验并压缩…"
            });
            ui.ctx().request_repaint();
        }
        // 先处理本帧输入，再使用结果。初次打开时输入字符也会使旧的自动格式化过期。
        self.update_worker(ui.ctx(), text_id, clipboard, worker);
    }

    fn toolbar(&self, ui: &mut Ui) -> Toolbar {
        egui::Panel::bottom("editor-footer")
            .resizable(false)
            .frame(Frame::new().fill(BACKGROUND).inner_margin(10))
            .show(ui, |ui| {
                let mut toolbar = Toolbar::default();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("压缩复制").clicked() {
                        toolbar.operation = Some(Operation::Compact);
                    }
                    if ui.button("格式化").clicked() {
                        toolbar.operation = Some(Operation::Format);
                    }
                    toolbar.paste = ui.button("粘贴").clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if let Some(diagnostic) = diagnostic(self.report.as_ref()) {
                            let label = RichText::new(format!(
                                "第 {} 行，第 {} 列",
                                diagnostic.line, diagnostic.column
                            ))
                            .color(json_view::ERROR)
                            .underline();
                            let response = ui
                                .add(
                                    egui::Label::new(label)
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text(format!(
                                    "{}\n{}",
                                    diagnostic.message, diagnostic.excerpt
                                ));
                            if response.clicked() {
                                toolbar.jump = Some(diagnostic.byte);
                            }
                        } else {
                            let label = match self.report.as_ref().map(|report| &report.issue) {
                                Some(Some(Issue::InputLimit)) => "输入超过 4 MiB",
                                Some(Some(Issue::FormatLimit)) => "格式化结果超过 4 MiB",
                                Some(Some(Issue::Syntax(_))) => "JSON 有语法错误",
                                Some(None) => "JSON 有效",
                                None if self.document.source.is_empty() => "请输入 JSON",
                                None if self.scheduled.is_some() || self.pending.is_some() => {
                                    "正在校验…"
                                }
                                None => "校验未完成",
                            };
                            ui.add(egui::Label::new(label).truncate())
                                .on_hover_text(label);
                        }
                    });
                });
                let caption = self.notice.map_or_else(
                    || self.status.to_owned(),
                    |notice| format!("{}  {}", self.status, notice),
                );
                ui.add(egui::Label::new(&caption).truncate())
                    .on_hover_text(caption);
                if let Some(diagnostic) = diagnostic(self.report.as_ref())
                    && !diagnostic.excerpt.is_empty()
                {
                    let excerpt = RichText::new(&diagnostic.excerpt)
                        .monospace()
                        .color(json_view::ERROR);
                    ui.add(egui::Label::new(excerpt).truncate())
                        .on_hover_text(&diagnostic.message);
                }
                toolbar
            })
            .inner
    }

    fn draw_document(&mut self, ui: &mut Ui, text_id: Id, jump: Option<usize>) -> bool {
        let pending_folds;
        let folds = if let Some(report) = &self.report {
            &report.folds
        } else {
            pending_folds = self.document.collapsed().to_vec();
            &pending_folds
        };
        let mut buffer = self.document.buffer();
        let output = egui::CentralPanel::default()
            .frame(Frame::new().fill(BACKGROUND).inner_margin(10))
            .show(ui, |ui| {
                Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, Color32::from_rgb(221, 224, 229)))
                    .corner_radius(4)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        json_view::show(
                            ui,
                            text_id,
                            &mut buffer,
                            folds,
                            diagnostic(self.report.as_ref()),
                            jump,
                        )
                    })
                    .inner
            })
            .inner;
        let changed = buffer.changed;
        let reproject = buffer.projection_changed || output.toggle.is_some();
        if let Some(selection) = output.selection {
            self.selection = selection;
        }
        if let Some(fold) = output.toggle {
            self.document.toggle(fold);
        }
        if reproject {
            restore_selection(ui.ctx(), text_id, &self.document, self.selection);
            ui.ctx().request_repaint();
        }
        changed
    }

    fn update_worker(&mut self, ctx: &Context, text_id: Id, clipboard: Clipboard, worker: &Worker) {
        if let Some(pending) = &self.pending {
            match pending.result.try_recv() {
                Ok(report) => {
                    let pending = self.pending.take().expect("pending JSON job");
                    if pending.serial == self.serial {
                        self.apply_report(ctx, text_id, clipboard, &pending, report);
                        ctx.request_repaint();
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.notice = Some("JSON 工作线程已中断。");
                    log::warn!("JSON 工作线程未返回结果");
                    ctx.request_repaint();
                }
            }
        }
        let Some(scheduled) = &self.scheduled else {
            return;
        };
        let remaining = scheduled.earliest.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            ctx.request_repaint_after(remaining);
            return;
        }
        match worker.submit(Arc::clone(&self.document.source), scheduled.operation, ctx) {
            Ok(result) => {
                self.pending = Some(Pending {
                    serial: self.serial,
                    operation: scheduled.operation,
                    initial: scheduled.initial,
                    result,
                });
                self.scheduled = None;
            }
            Err(SubmitError::Busy) => ctx.request_repaint_after(Duration::from_millis(30)),
            Err(SubmitError::Offline) => {
                self.scheduled = None;
                self.notice = Some("JSON 工作线程不可用。");
                ctx.request_repaint();
            }
        }
    }

    fn apply_report(
        &mut self,
        ctx: &Context,
        text_id: Id,
        clipboard: Clipboard,
        pending: &Pending,
        mut report: Report,
    ) {
        match report.output.take() {
            Some(Output::Formatted { text, repaired }) => {
                if *self.document.source != text {
                    self.history.record(self.snapshot());
                    self.document.replace_all(Arc::new(text));
                    self.selection = if pending.initial {
                        Selection::default()
                    } else {
                        Selection::caret(self.document.source.floor_char_boundary(
                            self.selection.primary.min(self.document.source.len()),
                        ))
                    };
                    restore_selection(ctx, text_id, &self.document, self.selection);
                }
                self.notice = if repaired {
                    Some("已转义字符串中的换行。")
                } else if pending.initial {
                    None
                } else {
                    Some("已格式化。")
                };
            }
            Some(Output::Compacted(text)) => {
                self.write_clipboard(clipboard, &text, "已将单行 JSON 复制到剪贴板。");
                self.notice = None;
            }
            None if pending.operation != Operation::Validate && !pending.initial => {
                self.notice = Some(if pending.operation == Operation::Format {
                    "格式化未完成。"
                } else {
                    "压缩复制未完成。"
                });
            }
            None => {}
        }
        if self.document.reconcile_folds(&report.folds) {
            restore_selection(ctx, text_id, &self.document, self.selection);
        }
        self.report = Some(report);
    }

    fn schedule(&mut self, operation: Operation, delay: Duration) {
        self.serial = self.serial.wrapping_add(1);
        self.scheduled = Some(Scheduled {
            operation,
            earliest: Instant::now() + delay,
            initial: false,
        });
    }

    fn document_changed(&mut self) {
        self.report = None;
        self.notice = None;
        self.schedule(Operation::Validate, Duration::from_millis(120));
        if self.document.source.is_empty() {
            self.scheduled = None;
        }
    }

    fn paste(&mut self, ui: &Ui, clipboard: Clipboard, id: Id) {
        self.status = match clipboard.read() {
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
        ui.ctx().request_repaint();
    }

    fn copy_selection(&mut self, ui: &Ui, clipboard: Clipboard, id: Id, cut: bool) -> bool {
        let range = self.selection.sorted();
        let source = Arc::clone(&self.document.source);
        let copied = self.write_clipboard(clipboard, &source[range.clone()], "已复制到剪贴板。");
        ui.ctx().request_repaint();
        if cut && copied {
            self.selection = Selection::caret(range.start);
            self.document.replace(range, "");
            restore_selection(ui.ctx(), id, &self.document, self.selection);
            return true;
        }
        false
    }

    fn write_clipboard(&mut self, clipboard: Clipboard, text: &str, success: &'static str) -> bool {
        match clipboard.write(text) {
            Ok(()) => {
                self.status = success;
                true
            }
            Err(error) => {
                log::warn!("编辑窗口写入剪贴板失败：{error}");
                self.status = "无法写入剪贴板，请重试。";
                false
            }
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: Arc::clone(&self.document.source),
            selection: self.selection,
        }
    }

    fn restore_history(&mut self, ctx: &Context, text_id: Id, action: HistoryAction) {
        let current = self.snapshot();
        if let Some(snapshot) = self.history.restore(current, action) {
            self.document.replace_all(snapshot.text);
            self.selection = snapshot.selection;
            restore_selection(ctx, text_id, &self.document, self.selection);
            self.document_changed();
        }
    }
}

fn diagnostic(report: Option<&Report>) -> Option<&Diagnostic> {
    match report?.issue.as_ref()? {
        Issue::Syntax(diagnostic) => Some(diagnostic),
        _ => None,
    }
}

fn restore_selection(ctx: &Context, id: Id, document: &Document, selection: Selection) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(document.display_selection(selection)));
    state.store(ctx, id);
}

fn shortcuts(ui: &Ui) -> Shortcuts {
    ui.input_mut(|input| {
        let command = egui::Modifiers::COMMAND;
        let redo = input.consume_key(command | egui::Modifiers::SHIFT, egui::Key::Z)
            || input.consume_key(command, egui::Key::Y);
        let undo = input.consume_key(command, egui::Key::Z);
        let mut shortcuts = Shortcuts {
            paste: input.consume_key(command, egui::Key::V),
            copy: input.consume_key(command, egui::Key::C),
            cut: input.consume_key(command, egui::Key::X),
            history: if redo {
                Some(HistoryAction::Redo)
            } else if undo {
                Some(HistoryAction::Undo)
            } else {
                None
            },
        };
        input.events.retain(|event| match event {
            egui::Event::Copy => {
                shortcuts.copy = true;
                false
            }
            egui::Event::Cut => {
                shortcuts.cut = true;
                false
            }
            _ => true,
        });
        shortcuts
    })
}

#[derive(Clone, Copy)]
enum HistoryAction {
    Undo,
    Redo,
}

struct Snapshot {
    text: Arc<String>,
    selection: Selection,
}

#[derive(Default)]
struct History {
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
}

impl History {
    fn record(&mut self, before: Snapshot) {
        Self::push(&mut self.undo, before);
        self.redo.clear();
    }

    fn restore(&mut self, current: Snapshot, action: HistoryAction) -> Option<Snapshot> {
        let (source, destination) = match action {
            HistoryAction::Undo => (&mut self.undo, &mut self.redo),
            HistoryAction::Redo => (&mut self.redo, &mut self.undo),
        };
        let snapshot = source.pop_back()?;
        Self::push(destination, current);
        Some(snapshot)
    }

    fn push(history: &mut VecDeque<Snapshot>, entry: Snapshot) {
        const MAX_BYTES: usize = 16 * 1024 * 1024;
        if entry.text.len() > MAX_BYTES {
            history.clear();
            return;
        }
        let mut bytes =
            history.iter().map(|entry| entry.text.len()).sum::<usize>() + entry.text.len();
        while bytes > MAX_BYTES || history.len() >= 64 {
            bytes -= history
                .pop_front()
                .expect("nonempty history over budget")
                .text
                .len();
        }
        history.push_back(entry);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use eframe::egui::{Event, FullOutput, Pos2, RawInput, Rect, Shape};
    use rpanel::core::json;

    fn frame(
        ctx: &Context,
        editor: &mut Editor,
        worker: &Worker,
        events: Vec<Event>,
    ) -> FullOutput {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(720.0, 480.0))),
                events,
                focused: true,
                ..Default::default()
            },
            |ui| editor.show(ui, Clipboard, worker),
        );
        output.textures_delta.clear();
        output
    }

    fn text_position(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<Pos2> {
        fn find(shape: &Shape, label: &str) -> Option<Pos2> {
            match shape {
                Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        shapes.iter().find_map(|shape| find(&shape.shape, label))
    }

    fn click(ctx: &Context, editor: &mut Editor, worker: &Worker, at: Pos2) {
        for pressed in [true, false] {
            frame(
                ctx,
                editor,
                worker,
                vec![
                    Event::PointerMoved(at),
                    Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn initial_format_cannot_replace_text_entered_in_the_result_frame() {
        let ctx = Context::default();
        let worker = Worker::default();
        let source = r#"{"x":1}"#;
        let mut editor = Editor::new(source.to_owned(), "");
        editor.scheduled = None;
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        editor.pending = Some(Pending {
            serial: editor.serial,
            operation: Operation::Format,
            initial: true,
            result,
        });
        sender
            .send(json::process(source, Operation::Format))
            .unwrap();

        frame(&ctx, &mut editor, &worker, vec![Event::Text("x".into())]);

        assert_eq!(editor.document.source.as_str(), format!("x{source}"));
        assert!(editor.pending.is_none());
    }

    #[test]
    fn clicking_gutter_and_diagnostic_expands_the_error_and_moves_the_caret() {
        let ctx = Context::default();
        let worker = Worker::default();
        let source = "{\n  \"k\": [\n    @\n  ]\n}";
        let mut editor = Editor::new(source.to_owned(), "");
        editor.scheduled = None;
        editor.report = Some(json::process(source, Operation::Validate));
        let report = editor.report.as_ref().unwrap();
        let outer = report.folds[0];
        let inner = report.folds[1];
        let error_byte = diagnostic(Some(report)).unwrap().byte;
        let rendered = frame(&ctx, &mut editor, &worker, Vec::new());
        let fold_button = text_position(&rendered.shapes, "▾").expect("visible gutter button");
        click(&ctx, &mut editor, &worker, fold_button);
        assert_eq!(editor.document.collapsed(), &[outer]);
        editor.document.toggle(inner);

        let rendered = frame(&ctx, &mut editor, &worker, Vec::new());
        let error_button =
            text_position(&rendered.shapes, "第 3 行，第 5 列").expect("visible error location");
        click(&ctx, &mut editor, &worker, error_button);
        assert!(editor.document.collapsed().is_empty());
        assert_eq!(editor.selection, Selection::caret(error_byte));
        assert_eq!(editor.document.source.as_str(), source);
    }

    #[test]
    fn compact_button_submits_the_complete_document_while_folded() {
        let ctx = Context::default();
        let worker = Worker::default();
        let source = "{\n  \"k\": [\n    123456789012345678901,\n    2\n  ]\n}";
        let mut editor = Editor::new(source.to_owned(), "");
        editor.scheduled = None;
        let report = json::process(source, Operation::Validate);
        editor.document.toggle(report.folds[0]);
        editor.report = Some(report);
        let rendered = frame(&ctx, &mut editor, &worker, Vec::new());
        let compact = text_position(&rendered.shapes, "压缩复制").expect("visible compact button");
        click(&ctx, &mut editor, &worker, compact);

        // 从工作线程取结果，避免测试写入宿主机的系统剪贴板。
        let pending = editor.pending.take().expect("compact job was submitted");
        let result = pending.result.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            matches!(result.output, Some(Output::Compacted(text)) if text == r#"{"k":[123456789012345678901,2]}"#)
        );
        assert_eq!(editor.document.source.as_str(), source);
    }

    #[test]
    fn formatting_a_folded_document_keeps_every_value_and_expands_the_result() {
        let ctx = Context::default();
        let worker = Worker::default();
        let source = "{\"k\": [\n  1,2\n]}";
        let mut editor = Editor::new(source.to_owned(), "");
        editor.scheduled = None;
        let report = json::process(source, Operation::Validate);
        editor.document.toggle(report.folds[0]);
        editor.report = Some(report);
        let rendered = frame(&ctx, &mut editor, &worker, Vec::new());
        let format = text_position(&rendered.shapes, "格式化").expect("visible format button");
        click(&ctx, &mut editor, &worker, format);

        let pending = editor.pending.as_mut().expect("format job was submitted");
        let report = pending.result.recv_timeout(Duration::from_secs(2)).unwrap();
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        sender.send(report).unwrap();
        pending.result = result;
        frame(&ctx, &mut editor, &worker, Vec::new());

        assert!(editor.document.collapsed().is_empty());
        assert_eq!(
            editor.document.source.as_str(),
            "{\n  \"k\": [\n    1,\n    2\n  ]\n}"
        );
    }
}
