use std::{ops::Range, sync::Arc};

use eframe::egui::{
    self, Color32, FontId, Frame, Id, Layout, Rect, Stroke, TextBuffer, Ui,
    text::{ByteIndex, LayoutJob, TextFormat, TextWrapping},
};
use rpanel::core::json::{self, Diagnostic, Fold, Kind};

use super::document::{Buffer, Selection};

const INK: Color32 = Color32::from_rgb(29, 29, 31);
pub const ERROR: Color32 = Color32::from_rgb(176, 35, 24);
const GUTTER_WIDTH: f32 = 18.0;
const HIGHLIGHT_TOKENS: usize = 40_000;

pub struct ViewOutput {
    pub selection: Option<Selection>,
    pub toggle: Option<Fold>,
}

pub fn show(
    ui: &mut Ui,
    id: Id,
    buffer: &mut Buffer<'_>,
    folds: &[Fold],
    diagnostic: Option<&Diagnostic>,
    jump: Option<usize>,
) -> ViewOutput {
    let height = ui.available_height();
    egui::ScrollArea::vertical()
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                let left = ui.next_widget_position().x;
                ui.add_space(GUTTER_WIDTH);
                let error = diagnostic.and_then(|diagnostic| buffer.display_byte(diagnostic.byte));
                let mut layouter = |ui: &Ui, buffer: &dyn TextBuffer, width| {
                    layout(ui, buffer.as_str(), width, error)
                };
                let mut output = egui::TextEdit::multiline(buffer)
                    .id(id)
                    .code_editor()
                    .font(FontId::monospace(14.0))
                    .frame(Frame::NONE)
                    .min_size(egui::vec2(ui.available_width(), height))
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter)
                    .text_color(INK)
                    .hint_text("粘贴或输入 JSON 文本")
                    .show(ui);

                if let Some(byte) = jump
                    && let Some(cursor) = buffer.source_position(byte)
                {
                    let rect = output
                        .galley
                        .pos_from_cursor(cursor)
                        .translate(output.galley_pos.to_vec2());
                    ui.scroll_to_rect(rect.expand(4.0), Some(egui::Align::Center));
                }
                // 撤销记录由完整文档持有，TextEdit 的历史只包含当前投影。
                output.state.clear_undoer();
                output.state.store(ui.ctx(), id);
                let selection = output
                    .cursor_range
                    .map(|range| buffer.source_selection(range));
                let toggle = if buffer.changed || buffer.projection_changed {
                    None
                } else {
                    gutter(
                        ui,
                        id,
                        buffer,
                        folds,
                        &output.galley,
                        output.galley_pos,
                        left,
                    )
                };
                ViewOutput { selection, toggle }
            })
            .inner
        })
        .inner
}

fn gutter(
    ui: &mut Ui,
    id: Id,
    buffer: &Buffer<'_>,
    folds: &[Fold],
    galley: &egui::Galley,
    origin: egui::Pos2,
    left: f32,
) -> Option<Fold> {
    if folds.is_empty() || galley.rows.is_empty() {
        return None;
    }
    let clip = ui.clip_rect();
    let top = galley
        .rows
        .partition_point(|row| origin.y + row.rect().bottom() < clip.top());
    let visible = galley.rows[top..]
        .iter()
        .take_while(|row| origin.y + row.rect().top() <= clip.bottom());
    let skipped = galley.rows[..top]
        .iter()
        .map(|row| row.char_count_including_newline().0)
        .sum::<usize>();
    let text = buffer.as_str();
    let mut byte = text
        .char_indices()
        .nth(skipped)
        .map_or(text.len(), |(i, _)| i);
    let mut clicked = None;
    for row in visible {
        let end = text[byte..]
            .char_indices()
            .nth(row.char_count_including_newline().0)
            .map_or(text.len(), |(i, _)| byte + i);
        let source_range = buffer.source_byte(byte)..buffer.source_byte(end);
        let mut start = folds.partition_point(|fold| fold.open < source_range.start);
        let mut candidates = Vec::new();
        while let Some(&fold) = folds.get(start).filter(|fold| fold.open < source_range.end) {
            if let Some(end) = buffer.hidden_end(fold.open) {
                start += folds[start..].partition_point(|fold| fold.open < end);
            } else {
                candidates.push(fold);
                start += 1;
            }
        }
        byte = end;
        if candidates.is_empty() {
            continue;
        }
        let center = egui::pos2(left + GUTTER_WIDTH / 2.0, origin.y + row.rect().center().y);
        let rect = Rect::from_center_size(center, egui::vec2(GUTTER_WIDTH, row.rect().height()));
        if let [fold] = candidates.as_slice() {
            let (glyph, label) = fold_label(buffer, *fold);
            let response = ui
                .push_id(id.with(fold.open), |ui| {
                    ui.put(rect, egui::Button::new(glyph).frame(false))
                })
                .inner;
            if response.on_hover_text(label).clicked() {
                clicked = Some(*fold);
            }
        } else {
            let response = ui
                .push_id(id.with(candidates[0].open), |ui| {
                    ui.put(rect, egui::Button::new("⋯").frame(false))
                })
                .inner;
            egui::Popup::menu(&response).show(|ui| {
                for (position, fold) in candidates.into_iter().enumerate() {
                    let (_, label) = fold_label(buffer, fold);
                    if ui
                        .button(format!("{}第 {} 个对象或数组", label, position + 1))
                        .clicked()
                    {
                        clicked = Some(fold);
                    }
                }
            });
        }
    }
    clicked
}

fn fold_label(buffer: &Buffer<'_>, fold: Fold) -> (&'static str, &'static str) {
    if buffer.is_collapsed(fold) {
        ("▸", "展开")
    } else {
        ("▾", "折叠")
    }
}

fn layout(ui: &Ui, source: &str, width: f32, error: Option<usize>) -> Arc<egui::Galley> {
    let mut job = LayoutJob {
        text: source.to_owned(),
        wrap: TextWrapping {
            max_width: width,
            ..Default::default()
        },
        keep_trailing_whitespace: true,
        ..Default::default()
    };
    let font = FontId::monospace(14.0);
    let error = error.and_then(|start| {
        let suffix = source.get(start..)?;
        if let Some(character) = suffix.chars().next().filter(|&character| character != '\n') {
            Some(start..start + character.len_utf8())
        } else {
            source
                .get(..start)?
                .char_indices()
                .next_back()
                .map(|(previous, _)| previous..start)
        }
    });
    let mut append = |range: Range<usize>, color: Color32| {
        let format = TextFormat::simple(font.clone(), color);
        if let Some(error) = error
            .as_ref()
            .filter(|error| error.start < range.end && error.end > range.start)
        {
            let start = range.start.max(error.start);
            let end = range.end.min(error.end);
            add_section(&mut job, range.start..start, format.clone());
            add_section(
                &mut job,
                start..end,
                TextFormat {
                    underline: Stroke::new(1.3, ERROR),
                    ..format.clone()
                },
            );
            add_section(&mut job, end..range.end, format);
        } else {
            add_section(&mut job, range, format);
        }
    };
    let mut offset = 0;
    if source.len() <= json::MAX_BYTES {
        for token in json::tokens(source).take(HIGHLIGHT_TOKENS) {
            append(offset..token.bytes.start, INK);
            offset = token.bytes.end;
            let color = match token.kind {
                Kind::Key => Color32::from_rgb(116, 64, 160),
                Kind::StringValue => Color32::from_rgb(35, 116, 76),
                Kind::Number => Color32::from_rgb(157, 83, 27),
                Kind::Boolean | Kind::Null => Color32::from_rgb(34, 85, 164),
                Kind::Other if &source[token.bytes.clone()] == "…" => {
                    Color32::from_rgb(96, 100, 108)
                }
                Kind::Other => INK,
                _ => Color32::from_rgb(96, 100, 108),
            };
            append(token.bytes, color);
        }
    }
    append(offset..source.len(), INK);
    if job.sections.is_empty() {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(0)..ByteIndex(0),
            format: TextFormat::simple(font, INK),
        });
    }
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn add_section(job: &mut LayoutJob, range: Range<usize>, format: TextFormat) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = job
        .sections
        .last_mut()
        .filter(|last| last.format == format && last.byte_range.end.0 == range.start)
    {
        last.byte_range.end = ByteIndex(range.end);
    } else {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(range.start)..ByteIndex(range.end),
            format,
        });
    }
}
