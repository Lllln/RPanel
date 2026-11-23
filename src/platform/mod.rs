#[cfg(target_os = "macos")]
mod macos;
mod selection;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{Clipboard, MouseListener, Popup, Trigger, configure};
pub use selection::{Selection, SelectionRequest, SelectionResult};
#[cfg(windows)]
pub use windows::{Clipboard, MouseListener, Popup, Trigger, configure};

#[derive(Clone, Copy)]
pub struct PanelBounds {
    pub generation: u64,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PanelBounds {
    fn contains(self, point: rpanel::gesture::Point) -> bool {
        (self.left..self.right).contains(&point.x) && (self.top..self.bottom).contains(&point.y)
    }
}

pub enum MouseEvent {
    Show(Trigger),
    Dismiss(u64),
    Listening(bool),
}
