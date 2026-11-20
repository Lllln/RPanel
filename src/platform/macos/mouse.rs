use std::{
    cell::{Cell, RefCell},
    io,
    ptr::NonNull,
    rc::Rc,
};

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSWorkspace};
use objc2_foundation::NSPoint;
use rpanel::gesture::{Point, RightHold};

use crate::platform::{MouseEvent, PanelBounds};

#[derive(Clone, Copy)]
pub struct Trigger {
    pub(super) point: Point,
    pub(super) foreground: Option<i32>,
}

struct Monitor(Retained<AnyObject>);

impl Drop for Monitor {
    fn drop(&mut self) {
        // SAFETY: token 来自 NSEvent 的注册方法，只在注册它的主线程移除一次。
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

struct State {
    hold: RightHold,
    foreground: Option<i32>,
}

pub struct MouseListener {
    state: Rc<RefCell<State>>,
    bounds: Rc<Cell<Option<PanelBounds>>>,
    emit: Rc<dyn Fn(MouseEvent)>,
    monitors: Option<(Monitor, Monitor)>,
    main_thread: MainThreadMarker,
}

impl MouseListener {
    pub fn start(threshold_ms: u32, emit: impl Fn(MouseEvent) + 'static) -> io::Result<Self> {
        let main_thread = MainThreadMarker::new()
            .ok_or_else(|| io::Error::other("AppKit 鼠标监听需要在主线程创建"))?;
        let mut listener = Self {
            state: Rc::new(RefCell::new(State {
                hold: RightHold::new(threshold_ms),
                foreground: None,
            })),
            bounds: Rc::new(Cell::new(None)),
            emit: Rc::new(emit),
            monitors: None,
            main_thread,
        };
        if let Err(error) = listener.set_enabled(true) {
            log::warn!("无法启动鼠标监听，仍可从菜单栏预览：{error}");
        }
        Ok(listener)
    }

    pub fn set_enabled(&mut self, enabled: bool) -> io::Result<()> {
        if !enabled {
            self.monitors = None;
            self.state.borrow_mut().hold.cancel();
        } else if self.monitors.is_none() {
            self.monitors = Some(self.register()?);
        }
        (self.emit)(MouseEvent::Listening(enabled));
        Ok(())
    }

    pub fn set_panel_bounds(&self, bounds: Option<PanelBounds>) {
        self.bounds.set(bounds);
    }

    fn register(&self) -> io::Result<(Monitor, Monitor)> {
        let down =
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
        let state = Rc::clone(&self.state);
        let bounds = Rc::clone(&self.bounds);
        let emit = Rc::clone(&self.emit);
        let main_thread = self.main_thread;
        let global = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: AppKit 在回调期间持有事件，回调在主线程同步执行。
            let event = unsafe { event.as_ref() };
            let kind = event.r#type();
            if kind == NSEventType::ScrollWheel {
                state.borrow_mut().hold.cancel();
                return;
            }
            let point = screen_point(event, main_thread);
            dismiss_if_outside(bounds.get(), down_event(kind), point, &*emit);
            let at_ms = (event.timestamp() * 1_000.0) as u64 as u32;
            let trigger = {
                let mut state = state.borrow_mut();
                match kind {
                    NSEventType::RightMouseDown => {
                        state.foreground = frontmost_pid();
                        state.hold.press(at_ms, point, 8);
                        None
                    }
                    NSEventType::RightMouseDragged => {
                        state.hold.move_to(point);
                        None
                    }
                    NSEventType::RightMouseUp if state.hold.release(at_ms, point) => {
                        state.foreground.map(|pid| Trigger {
                            point,
                            foreground: Some(pid),
                        })
                    }
                    NSEventType::LeftMouseDown | NSEventType::OtherMouseDown => {
                        state.hold.cancel();
                        None
                    }
                    _ => None,
                }
            };
            if let Some(trigger) = trigger {
                emit(MouseEvent::Show(trigger));
            }
        });
        let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            down | NSEventMask::RightMouseUp
                | NSEventMask::RightMouseDragged
                | NSEventMask::ScrollWheel,
            &global,
        )
        .map(Monitor)
        .ok_or_else(|| io::Error::other("AppKit 未注册全局鼠标观察器"))?;

        let state = Rc::clone(&self.state);
        let bounds = Rc::clone(&self.bounds);
        let emit = Rc::clone(&self.emit);
        let local = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: AppKit 保证入参有效，处理后将原事件指针交还给 AppKit。
            let borrowed = unsafe { event.as_ref() };
            state.borrow_mut().hold.cancel();
            dismiss_if_outside(
                bounds.get(),
                down_event(borrowed.r#type()),
                screen_point(borrowed, main_thread),
                &*emit,
            );
            event.as_ptr()
        });
        // SAFETY: 回调只在 AppKit 主线程访问状态，并总是返回同一个有效事件指针。
        let local = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                down | NSEventMask::RightMouseUp,
                &local,
            )
        }
        .map(Monitor)
        .ok_or_else(|| io::Error::other("AppKit 未注册本地鼠标观察器"))?;
        Ok((global, local))
    }
}

fn down_event(kind: NSEventType) -> bool {
    matches!(
        kind,
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown
    )
}

fn dismiss_if_outside(
    bounds: Option<PanelBounds>,
    down: bool,
    point: Point,
    emit: &dyn Fn(MouseEvent),
) {
    if down
        && let Some(bounds) = bounds
        && !bounds.contains(point)
    {
        emit(MouseEvent::Dismiss(bounds.generation));
    }
}

fn screen_point(event: &NSEvent, main_thread: MainThreadMarker) -> Point {
    let point = event.locationInWindow();
    let point = event
        .window(main_thread)
        .map_or(point, |window| window.convertPointToScreen(point));
    appkit_point(point)
}

pub(super) fn appkit_point(point: NSPoint) -> Point {
    // AppKit 的屏幕 Y 向上，统一到 RPanel 自上向下的坐标。
    Point {
        x: point.x.floor() as i32,
        y: (-point.y).floor() as i32,
    }
}

pub(super) fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
        .filter(|pid| *pid > 0)
}
