use std::io;

use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSEvent, NSFloatingWindowLevel, NSScreen, NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::RawWindowHandle;
use rpanel::{gesture::Point, placement::axis_position};

use super::{
    Clipboard,
    mouse::{Trigger, appkit_point, frontmost_pid},
};
use crate::platform::PanelBounds;

pub struct Popup {
    window: Retained<NSWindow>,
    main_thread: MainThreadMarker,
}

impl Popup {
    pub fn attach(handle: RawWindowHandle) -> io::Result<Self> {
        let RawWindowHandle::AppKit(handle) = handle else {
            return Err(io::Error::other("eframe 未提供 AppKit 窗口句柄"));
        };
        let main_thread = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("AppKit 窗口需要在主线程使用"))?;
        // SAFETY: 句柄借自 eframe 当前的 AppKit 窗口。仅在 GUI 线程访问有效 NSView。
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view
            .window()
            .ok_or_else(|| io::Error::other("无法取得 eframe 的 NSWindow"))?;
        window.setLevel(NSFloatingWindowLevel);
        window.setHasShadow(true);
        Ok(Self {
            window,
            main_thread,
        })
    }

    pub fn preview() -> Option<Trigger> {
        Some(Trigger {
            point: appkit_point(NSEvent::mouseLocation()),
            foreground: None,
        })
    }

    pub fn clipboard(&self) -> Clipboard {
        Clipboard
    }

    pub fn show(
        &self,
        trigger: Trigger,
        size: [u32; 2],
        generation: u64,
        before_show: impl FnOnce(),
    ) -> io::Result<Option<PanelBounds>> {
        if let Some(pid) = trigger.foreground
            && Some(pid) != frontmost_pid()
        {
            return Ok(None);
        }
        let screens = NSScreen::screens(self.main_thread);
        let point = trigger.point;
        let screen = (0..screens.count())
            .map(|index| screens.objectAtIndex(index))
            .find(|screen| bounds_from_frame(screen.frame(), 0).contains(point));
        let Some(screen) = screen else {
            return Ok(None);
        };
        // AppKit 和 egui 的逻辑尺寸均以点计。Retina 的像素缩放交给宿主窗口。
        let work = bounds_from_frame(screen.visibleFrame(), generation);
        let width = (size[0] as i32).min(work.right - work.left);
        let height = (size[1] as i32).min(work.bottom - work.top);
        if width <= 0 || height <= 0 {
            return Ok(None);
        }
        let x = axis_position(point.x, work.left, work.right, width, 10);
        let y = axis_position(point.y, work.top, work.bottom, height, 10);
        let frame = NSRect::new(
            NSPoint::new(x as f64, (-y - height) as f64),
            NSSize::new(width as f64, height as f64),
        );
        self.window.setFrame_display(frame, false);
        // 排在当前窗口层级前方，不调用 makeKeyAndOrderFront 或 activate。
        before_show();
        self.window.orderFrontRegardless();
        Ok(Some(bounds_from_frame(self.window.frame(), generation)))
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }
}

fn bounds_from_frame(frame: NSRect, generation: u64) -> PanelBounds {
    let top_left = Point {
        x: frame.origin.x.ceil() as i32,
        y: (-(frame.origin.y + frame.size.height)).ceil() as i32,
    };
    PanelBounds {
        generation,
        left: top_left.x,
        top: top_left.y,
        right: (frame.origin.x + frame.size.width).floor() as i32,
        bottom: (-frame.origin.y).floor() as i32,
    }
}
