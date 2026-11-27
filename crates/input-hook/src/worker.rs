use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use tracing::{debug, warn};

use crate::clipboard::AutoCopyBaseline;
use crate::state_machine::{LongPressStateMachine, StateAction};
use crate::{CaptureResult, CaptureSource, HookEvent, ScreenPoint};

/// Non-blocking commands sent from the low-level OS hook callback to the worker thread.
#[derive(Debug, Clone, Copy)]
pub enum WorkerCommand {
    ArmTimer {
        generation: u64,
        deadline: Instant,
    },
    CancelTimer,
    ReplayShortClick {
        down_at: ScreenPoint,
        up_at: ScreenPoint,
    },
    ReplayDragStart {
        down_at: ScreenPoint,
        current: ScreenPoint,
    },
    ReplayDragEnd {
        up_at: ScreenPoint,
    },
    TriggerLongPress {
        position: ScreenPoint,
    },
    LeftMouseDown {
        at: Instant,
    },
    GlobalMouseDown {
        position: ScreenPoint,
        at: Instant,
    },
    Shutdown,
}

/// Platform hooks implement these side-effect callbacks executed strictly off the OS hook thread.
pub trait PlatformActions: Send + 'static {
    fn replay_short_click(&self, down_at: ScreenPoint, up_at: ScreenPoint);
    fn replay_drag_start(&self, down_at: ScreenPoint, current: ScreenPoint);
    fn replay_drag_end(&self, up_at: ScreenPoint);
    fn capture_selection(&self) -> CaptureResult;

    /// Query the OS clipboard change counter for auto-copy-on-select tracking.
    fn clipboard_sequence_number(&self) -> Option<u64> {
        None
    }

    /// Capture selection with an optional pre-selection clipboard baseline recorded at `LeftMouseDown`.
    fn capture_selection_with_baseline(
        &self,
        _auto_copy_baseline: Option<AutoCopyBaseline>,
    ) -> CaptureResult {
        self.capture_selection()
    }
}

/// Spawn the dedicated worker thread handling hold timers, event replay, and selection capture.
pub fn spawn_worker<A: PlatformActions>(
    state: Arc<Mutex<LongPressStateMachine>>,
    event_tx: Sender<HookEvent>,
    actions: A,
) -> (Sender<WorkerCommand>, thread::JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = unbounded::<WorkerCommand>();

    let handle = thread::Builder::new()
        .name("rpanel-hook-worker".into())
        .spawn(move || worker_loop(cmd_rx, state, event_tx, actions))
        .expect("failed to spawn rpanel-hook-worker thread");

    (cmd_tx, handle)
}

fn worker_loop<A: PlatformActions>(
    cmd_rx: Receiver<WorkerCommand>,
    state: Arc<Mutex<LongPressStateMachine>>,
    event_tx: Sender<HookEvent>,
    actions: A,
) {
    let mut armed: Option<(u64, Instant)> = None;
    let mut last_left_down: Option<AutoCopyBaseline> = None;

    loop {
        let msg = match armed {
            Some((generation, deadline)) => match cmd_rx.recv_deadline(deadline) {
                Ok(cmd) => cmd,
                Err(RecvTimeoutError::Timeout) => {
                    armed = None;
                    let triggered = {
                        let mut sm = state.lock().unwrap_or_else(|e| e.into_inner());
                        sm.on_timer_fired(generation, Instant::now())
                    };
                    if let Some(StateAction::TriggerLongPress { position }) = triggered {
                        emit_long_press(position, last_left_down, &event_tx, &actions);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match cmd_rx.recv() {
                Ok(cmd) => cmd,
                Err(_) => break,
            },
        };

        match msg {
            WorkerCommand::ArmTimer {
                generation,
                deadline,
            } => {
                armed = Some((generation, deadline));
            }
            WorkerCommand::CancelTimer => {
                armed = None;
            }
            WorkerCommand::ReplayShortClick { down_at, up_at } => {
                armed = None;
                debug!(?down_at, ?up_at, "replaying native right short click");
                actions.replay_short_click(down_at, up_at);
            }
            WorkerCommand::ReplayDragStart { down_at, current } => {
                armed = None;
                debug!(?down_at, ?current, "replaying native right drag start");
                actions.replay_drag_start(down_at, current);
            }
            WorkerCommand::ReplayDragEnd { up_at } => {
                armed = None;
                debug!(?up_at, "replaying native right drag end");
                actions.replay_drag_end(up_at);
            }
            WorkerCommand::TriggerLongPress { position } => {
                armed = None;
                emit_long_press(position, last_left_down, &event_tx, &actions);
            }
            WorkerCommand::LeftMouseDown { at } => {
                if let Some(seq) = actions.clipboard_sequence_number() {
                    last_left_down = Some(AutoCopyBaseline { seq, at });
                }
            }
            WorkerCommand::GlobalMouseDown { position, at } => {
                if let Err(err) = event_tx.send(HookEvent::GlobalMouseDown { position, at }) {
                    warn!("failed to deliver GlobalMouseDown to receiver: {err}");
                }
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

fn emit_long_press<A: PlatformActions>(
    position: ScreenPoint,
    last_left_down: Option<AutoCopyBaseline>,
    event_tx: &Sender<HookEvent>,
    actions: &A,
) {
    debug!(?position, "right-button long-press triggered");
    let captured = actions.capture_selection_with_baseline(last_left_down);
    if let Err(err) = event_tx.send(HookEvent::LongPressTriggered { position, captured }) {
        warn!("failed to deliver HookEvent to receiver: {err}");
    }
}

/// Helper converting fallible capture into a safe `CaptureResult`.
pub fn capture_or_none(result: Result<CaptureResult, crate::HookError>) -> CaptureResult {
    match result {
        Ok(captured) => captured,
        Err(err) => {
            warn!("selection capture failed: {err}");
            CaptureResult {
                text: None,
                source: CaptureSource::None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::HookConfig;

    #[derive(Clone)]
    struct MockActions {
        short_clicks: Arc<AtomicUsize>,
        drag_starts: Arc<AtomicUsize>,
        drag_ends: Arc<AtomicUsize>,
        captures: Arc<AtomicUsize>,
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl MockActions {
        fn new() -> Self {
            Self {
                short_clicks: Arc::new(AtomicUsize::new(0)),
                drag_starts: Arc::new(AtomicUsize::new(0)),
                drag_ends: Arc::new(AtomicUsize::new(0)),
                captures: Arc::new(AtomicUsize::new(0)),
                order: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl PlatformActions for MockActions {
        fn replay_short_click(&self, _down_at: ScreenPoint, _up_at: ScreenPoint) {
            self.short_clicks.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push("short_click");
        }

        fn replay_drag_start(&self, _down_at: ScreenPoint, _current: ScreenPoint) {
            self.drag_starts.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push("drag_start");
        }

        fn replay_drag_end(&self, _up_at: ScreenPoint) {
            self.drag_ends.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push("drag_end");
        }

        fn capture_selection(&self) -> CaptureResult {
            self.captures.fetch_add(1, Ordering::SeqCst);
            CaptureResult {
                text: Some("{\"ok\":true}".into()),
                source: CaptureSource::ClipboardShortcut,
            }
        }
    }

    #[test]
    fn worker_fires_long_press_after_hold_deadline() {
        let config = HookConfig {
            hold_duration: Duration::from_millis(40),
            max_move_px: 8,
        };
        let state = Arc::new(Mutex::new(LongPressStateMachine::new(config)));
        let (event_tx, event_rx) = unbounded();
        let actions = MockActions::new();
        let (cmd_tx, worker) = spawn_worker(Arc::clone(&state), event_tx, actions.clone());

        let now = Instant::now();
        let action = state.lock().unwrap().on_right_down(ScreenPoint::new(300, 400), now);
        let StateAction::SwallowAndArmTimer {
            generation,
            hold_duration,
        } = action
        else {
            panic!("expected SwallowAndArmTimer");
        };

        cmd_tx
            .send(WorkerCommand::ArmTimer {
                generation,
                deadline: now + hold_duration,
            })
            .unwrap();

        let event = event_rx
            .recv_timeout(Duration::from_millis(300))
            .expect("worker should emit LongPressTriggered");
        assert_eq!(
            event,
            HookEvent::LongPressTriggered {
                position: ScreenPoint::new(300, 400),
                captured: CaptureResult {
                    text: Some("{\"ok\":true}".into()),
                    source: CaptureSource::ClipboardShortcut,
                },
            }
        );
        assert_eq!(actions.captures.load(Ordering::SeqCst), 1);

        // Right-up after long-press must be swallowed.
        assert_eq!(
            state
                .lock()
                .unwrap()
                .on_right_up(ScreenPoint::new(300, 400), Instant::now()),
            StateAction::Swallow
        );

        cmd_tx.send(WorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn worker_cancels_timer_on_drag_and_replays_drag_start() {
        let config = HookConfig {
            hold_duration: Duration::from_millis(50),
            max_move_px: 8,
        };
        let state = Arc::new(Mutex::new(LongPressStateMachine::new(config)));
        let (event_tx, event_rx) = unbounded();
        let actions = MockActions::new();
        let (cmd_tx, worker) = spawn_worker(Arc::clone(&state), event_tx, actions.clone());

        let now = Instant::now();
        let StateAction::SwallowAndArmTimer {
            generation,
            hold_duration,
        } = state.lock().unwrap().on_right_down(ScreenPoint::new(10, 10), now)
        else {
            panic!("expected SwallowAndArmTimer");
        };
        cmd_tx
            .send(WorkerCommand::ArmTimer {
                generation,
                deadline: now + hold_duration,
            })
            .unwrap();

        let drag = state.lock().unwrap().on_mouse_move(ScreenPoint::new(40, 10));
        assert_eq!(
            drag,
            StateAction::ReplayDragStart {
                down_at: ScreenPoint::new(10, 10),
                current: ScreenPoint::new(40, 10),
            }
        );
        cmd_tx
            .send(WorkerCommand::ReplayDragStart {
                down_at: ScreenPoint::new(10, 10),
                current: ScreenPoint::new(40, 10),
            })
            .unwrap();

        let up = state
            .lock()
            .unwrap()
            .on_right_up(ScreenPoint::new(45, 10), Instant::now());
        assert_eq!(
            up,
            StateAction::ReplayDragEnd {
                up_at: ScreenPoint::new(45, 10),
            }
        );
        cmd_tx
            .send(WorkerCommand::ReplayDragEnd {
                up_at: ScreenPoint::new(45, 10),
            })
            .unwrap();

        assert!(event_rx.recv_timeout(Duration::from_millis(120)).is_err());
        assert_eq!(actions.drag_starts.load(Ordering::SeqCst), 1);
        assert_eq!(actions.drag_ends.load(Ordering::SeqCst), 1);
        assert_eq!(actions.captures.load(Ordering::SeqCst), 0);
        assert_eq!(*actions.order.lock().unwrap(), vec!["drag_start", "drag_end"]);

        cmd_tx.send(WorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }
}
