mod mouse;
mod popup;

pub use mouse::{MouseListener, Trigger};
pub use popup::Popup;

pub fn configure(_options: &mut eframe::NativeOptions) {
    popup::set_dpi_awareness();
}
