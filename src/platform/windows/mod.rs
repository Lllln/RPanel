mod clipboard;
mod mouse;
mod popup;
pub(super) mod selection;

pub use clipboard::Clipboard;
pub use mouse::{MouseListener, Trigger};
pub use popup::Popup;

pub fn configure(_options: &mut eframe::NativeOptions) {
    popup::set_dpi_awareness();
}
