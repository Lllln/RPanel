#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{MouseListener, Popup, Trigger, configure};
#[cfg(windows)]
pub use windows::{MouseListener, Popup, Trigger, configure};

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
