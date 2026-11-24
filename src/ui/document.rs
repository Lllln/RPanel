use std::{any::TypeId, ops::Range, sync::Arc};

use eframe::egui::{
    TextBuffer,
    epaint::text::CharIndex,
    text::{CCursor, CCursorRange},
};
use rpanel::core::json::Fold;

const MARKER: &str = "…";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub primary: usize,
    pub secondary: usize,
}

impl Selection {
    pub fn caret(byte: usize) -> Self {
        Self {
            primary: byte,
            secondary: byte,
        }
    }

    pub fn sorted(self) -> Range<usize> {
        self.primary.min(self.secondary)..self.primary.max(self.secondary)
    }
}

pub struct Document {
    pub source: Arc<String>,
    collapsed: Vec<Fold>,
}

impl Document {
    pub fn new(text: String) -> Self {
        Self {
            source: Arc::new(text),
            collapsed: Vec::new(),
        }
    }

    pub fn replace_all(&mut self, text: Arc<String>) {
        self.source = text;
        self.collapsed.clear();
    }

    pub fn toggle(&mut self, fold: Fold) {
        match self.collapsed.binary_search(&fold) {
            Ok(index) => {
                self.collapsed.remove(index);
            }
            Err(index) => self.collapsed.insert(index, fold),
        }
    }

    pub fn collapsed(&self) -> &[Fold] {
        &self.collapsed
    }

    pub fn reconcile_folds(&mut self, folds: &[Fold]) -> bool {
        let before = self.collapsed.len();
        self.collapsed
            .retain(|fold| folds.binary_search(fold).is_ok());
        self.collapsed.len() != before
    }

    pub fn reveal(&mut self, byte: usize) {
        self.collapsed
            .retain(|fold| !fold.interior().contains(&byte));
    }

    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        Arc::make_mut(&mut self.source).replace_range(range.clone(), text);
        self.collapsed.retain_mut(|fold| {
            if range.end <= fold.open {
                let shift = text.len() as isize - range.len() as isize;
                fold.open = fold.open.wrapping_add_signed(shift);
                fold.close = fold.close.wrapping_add_signed(shift);
                true
            } else {
                range.start > fold.close
            }
        });
    }

    pub fn buffer(&mut self) -> Buffer<'_> {
        let view = View::new(&self.source, &self.collapsed);
        Buffer {
            document: self,
            view,
            changed: false,
            projection_changed: false,
        }
    }

    pub fn display_selection(&self, selection: Selection) -> CCursorRange {
        let view = View::new(&self.source, &self.collapsed);
        let text = view
            .as_ref()
            .map_or(self.source.as_str(), |view| &view.text);
        let cursor = |byte: usize| {
            let byte = self.source.floor_char_boundary(byte.min(self.source.len()));
            let displayed = view.as_ref().map_or(byte, |view| {
                view.to_display(byte).unwrap_or_else(|anchor| anchor)
            });
            CCursor::new(text[..displayed].chars().count())
        };
        CCursorRange::two(cursor(selection.secondary), cursor(selection.primary))
    }
}

struct Hidden {
    source: Range<usize>,
    display: Range<usize>,
}

struct View {
    text: String,
    hidden: Vec<Hidden>,
}

impl View {
    fn new(source: &str, collapsed: &[Fold]) -> Option<Self> {
        if collapsed.is_empty() {
            return None;
        }
        let mut shown = String::new();
        let mut hidden = Vec::new();
        let mut start = 0;
        for fold in collapsed {
            if fold.open < start {
                continue; // 父级收起时只显示父级的占位符。
            }
            let range = fold.interior();
            shown.push_str(&source[start..range.start]);
            let marker_start = shown.len();
            shown.push_str(MARKER);
            hidden.push(Hidden {
                source: range.clone(),
                display: marker_start..shown.len(),
            });
            start = range.end;
        }
        shown.push_str(&source[start..]);
        Some(Self {
            text: shown,
            hidden,
        })
    }

    fn to_source(&self, byte: usize) -> usize {
        let index = self
            .hidden
            .partition_point(|hidden| hidden.display.end <= byte);
        index.checked_sub(1).map_or(byte, |previous| {
            let hidden = &self.hidden[previous];
            hidden.source.end + byte - hidden.display.end
        })
    }

    // 被隐藏的源码偏移以占位符起点返回，供折叠后的光标定位使用。
    fn to_display(&self, byte: usize) -> Result<usize, usize> {
        let index = self
            .hidden
            .partition_point(|hidden| hidden.source.end <= byte);
        if let Some(hidden) = self.hidden.get(index)
            && byte > hidden.source.start
        {
            return Err(hidden.display.start);
        }
        Ok(index.checked_sub(1).map_or(byte, |previous| {
            let hidden = &self.hidden[previous];
            hidden.display.end + byte - hidden.source.end
        }))
    }

    fn replace(&mut self, range: Range<usize>, source_range: Range<usize>, text: &str) {
        let display_shift = text.len() as isize - range.len() as isize;
        let source_shift = text.len() as isize - source_range.len() as isize;
        self.hidden.retain_mut(|hidden| {
            if range.end <= hidden.display.start {
                hidden.display = hidden.display.start.wrapping_add_signed(display_shift)
                    ..hidden.display.end.wrapping_add_signed(display_shift);
                hidden.source = hidden.source.start.wrapping_add_signed(source_shift)
                    ..hidden.source.end.wrapping_add_signed(source_shift);
                true
            } else {
                range.start >= hidden.display.end
            }
        });
        self.text.replace_range(range, text);
    }
}

pub struct Buffer<'a> {
    document: &'a mut Document,
    view: Option<View>,
    pub changed: bool,
    pub projection_changed: bool,
}

impl Buffer<'_> {
    pub fn source_byte(&self, display: usize) -> usize {
        self.view
            .as_ref()
            .map_or(display, |view| view.to_source(display))
    }

    pub fn display_byte(&self, source: usize) -> Option<usize> {
        self.view
            .as_ref()
            .map_or(Some(source), |view| view.to_display(source).ok())
    }

    pub fn hidden_end(&self, source: usize) -> Option<usize> {
        let view = self.view.as_ref()?;
        let next = view
            .hidden
            .partition_point(|hidden| hidden.source.end <= source);
        view.hidden
            .get(next)
            .filter(|hidden| hidden.source.start < source)
            .map(|hidden| hidden.source.end)
    }

    pub fn source_selection(&self, range: CCursorRange) -> Selection {
        let byte =
            |cursor: CCursor| self.source_byte(self.byte_index_from_char_index(cursor.index).0);
        Selection {
            primary: byte(range.primary),
            secondary: byte(range.secondary),
        }
    }

    pub fn is_collapsed(&self, fold: Fold) -> bool {
        self.document.collapsed.binary_search(&fold).is_ok()
    }

    pub fn source_position(&self, byte: usize) -> Option<CCursor> {
        let byte = self.display_byte(byte)?;
        Some(CCursor::new(self.as_str().get(..byte)?.chars().count()))
    }

    fn replace_display(&mut self, range: Range<usize>, text: &str) {
        if range.is_empty() && text.is_empty() {
            return;
        }
        let source_range = self.source_byte(range.start)..self.source_byte(range.end);
        if text.is_empty()
            && self
                .view
                .as_ref()
                .is_some_and(|view| view.hidden.iter().any(|hidden| hidden.display == range))
        {
            // 单独删占位符相当于展开。显式跨范围编辑或剪切仍按完整文档处理。
            self.document.reveal(source_range.start);
            self.projection_changed = true;
            return;
        }
        let collapsed_before = self.document.collapsed.len();
        self.document.replace(source_range.clone(), text);
        self.projection_changed |= collapsed_before != self.document.collapsed.len();
        if let Some(view) = &mut self.view {
            view.replace(range, source_range, text);
        }
        self.changed = true;
    }
}

impl TextBuffer for Buffer<'_> {
    fn type_id(&self) -> TypeId {
        TypeId::of::<Buffer<'static>>()
    }

    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        self.view
            .as_ref()
            .map_or(self.document.source.as_str(), |view| view.text.as_str())
    }

    fn insert_text(&mut self, text: &str, position: CharIndex) -> usize {
        let byte = self.byte_index_from_char_index(position).0;
        self.replace_display(byte..byte, text);
        text.chars().count()
    }

    fn delete_char_range(&mut self, range: Range<CharIndex>) {
        let start = self.byte_index_from_char_index(range.start).0;
        let end = self.byte_index_from_char_index(range.end).0;
        self.replace_display(start..end, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpanel::core::json::{Operation, process};

    #[test]
    fn nested_folds_and_error_reveal_preserve_source_selection() {
        let source = "{\n  \"字\": [\n    @\n  ]\n}";
        let report = process(source, Operation::Validate);
        let mut document = Document::new(source.to_owned());
        let [outer, inner] = report.folds.as_slice() else {
            panic!("expected two folds");
        };
        document.toggle(*inner);
        document.toggle(*outer);
        let mut buffer = document.buffer();
        assert_eq!(buffer.as_str(), "{…}");
        assert_eq!(
            buffer
                .source_selection(CCursorRange::two(CCursor::new(1), CCursor::new(2)))
                .sorted(),
            outer.interior()
        );
        buffer.delete_char_range(CharIndex(1)..CharIndex(2));
        assert!(!buffer.changed);
        assert!(buffer.projection_changed);
        assert_eq!(document.buffer().as_str(), "{\n  \"字\": […]\n}");
        document.reveal(source.find('@').unwrap());
        assert_eq!(document.buffer().as_str(), source);
    }

    #[test]
    fn edits_rebase_unaffected_folds_and_copy_maps_to_the_complete_range() {
        let source = "[\n  [\n    \"🚀\"\n  ],\n  0\n]";
        let report = process(source, Operation::Validate);
        let mut document = Document::new(source.to_owned());
        document.toggle(report.folds[1]);
        let mut buffer = document.buffer();
        buffer.insert_text(" \n", CharIndex(0));
        let all_chars = buffer.as_str().chars().count();
        let selected =
            buffer.source_selection(CCursorRange::two(CCursor::new(0), CCursor::new(all_chars)));
        assert_eq!(selected.sorted(), 0..source.len() + 2);
        assert!(buffer.is_collapsed(Fold {
            open: report.folds[1].open + 2,
            close: report.folds[1].close + 2
        }));
        assert_eq!(document.source.as_str(), format!(" \n{source}"));
        assert!(document.buffer().as_str().contains("[…]"));
    }

    #[test]
    fn editing_a_bracket_expands_the_affected_fold_without_losing_hidden_text() {
        let source = "[\n  1,\n  2\n]";
        let report = process(source, Operation::Validate);
        let mut document = Document::new(source.to_owned());
        document.toggle(report.folds[0]);
        let mut buffer = document.buffer();
        buffer.delete_char_range(CharIndex(0)..CharIndex(1));
        assert!(buffer.changed && buffer.projection_changed);
        assert_eq!(document.buffer().as_str(), &source[1..]);
    }
}
