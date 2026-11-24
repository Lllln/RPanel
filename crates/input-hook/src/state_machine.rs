use std::time::{Duration, Instant};

use crate::{HookConfig, ScreenPoint};

/// Result of feeding a low-level right-button / pointer event into the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateAction {
    /// Pass the event unmodified to the OS / next hook.
    PassThrough,
    /// Swallow the event (suppress default OS right-click menu).
    Swallow,
    /// Swallow the press and schedule a long-press timer for `generation`.
    SwallowAndArmTimer {
        generation: u64,
        hold_duration: Duration,
    },
    /// Swallow this event and emit a synthetic short-click (down + up) at the original point.
    ReplayShortClick {
        down_at: ScreenPoint,
        up_at: ScreenPoint,
    },
    /// Swallow this event and transition into a real drag by replaying down at `down_at`.
    ReplayDragStart {
        down_at: ScreenPoint,
        current: ScreenPoint,
    },
    /// Long-press threshold reached while held stationary: swallow menu and trigger panel.
    TriggerLongPress { position: ScreenPoint },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Pending {
        generation: u64,
        down_at: ScreenPoint,
        pressed_at: Instant,
    },
    Triggered,
    Dragging,
}

/// Pure, platform-agnostic state machine for right-button long-press detection.
#[derive(Debug)]
pub struct LongPressStateMachine {
    config: HookConfig,
    phase: Phase,
    next_generation: u64,
}

impl LongPressStateMachine {
    pub fn new(config: HookConfig) -> Self {
        Self {
            config,
            phase: Phase::Idle,
            next_generation: 0,
        }
    }

    pub fn set_hold_duration(&mut self, duration: Duration) {
        self.config.hold_duration = duration;
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.phase, Phase::Pending { .. })
    }

    pub fn on_right_down(&mut self, pos: ScreenPoint, now: Instant) -> StateAction {
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.phase = Phase::Pending {
            generation,
            down_at: pos,
            pressed_at: now,
        };
        StateAction::SwallowAndArmTimer {
            generation,
            hold_duration: self.config.hold_duration,
        }
    }

    pub fn on_mouse_move(&mut self, pos: ScreenPoint) -> StateAction {
        let max_sq = i64::from(self.config.max_move_px) * i64::from(self.config.max_move_px);
        match self.phase {
            Phase::Pending { down_at, .. } if down_at.distance_squared(pos) > max_sq => {
                self.phase = Phase::Dragging;
                StateAction::ReplayDragStart {
                    down_at,
                    current: pos,
                }
            }
            _ => StateAction::PassThrough,
        }
    }

    pub fn on_timer_fired(&mut self, expected_generation: u64, now: Instant) -> Option<StateAction> {
        match self.phase {
            Phase::Pending {
                generation,
                down_at,
                pressed_at,
            } if generation == expected_generation
                && now.saturating_duration_since(pressed_at) >= self.config.hold_duration =>
            {
                self.phase = Phase::Triggered;
                Some(StateAction::TriggerLongPress { position: down_at })
            }
            _ => None,
        }
    }

    pub fn on_right_up(&mut self, pos: ScreenPoint, now: Instant) -> StateAction {
        let prev = std::mem::replace(&mut self.phase, Phase::Idle);
        match prev {
            Phase::Pending {
                down_at,
                pressed_at,
                ..
            } => {
                if now.saturating_duration_since(pressed_at) >= self.config.hold_duration {
                    StateAction::TriggerLongPress { position: down_at }
                } else {
                    StateAction::ReplayShortClick {
                        down_at,
                        up_at: pos,
                    }
                }
            }
            Phase::Triggered => StateAction::Swallow,
            Phase::Dragging | Phase::Idle => StateAction::PassThrough,
        }
    }

    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_click_replays_native_click() {
        let mut sm = LongPressStateMachine::new(HookConfig::default());
        let t0 = Instant::now();
        assert_eq!(
            sm.on_right_down(ScreenPoint::new(100, 100), t0),
            StateAction::SwallowAndArmTimer {
                generation: 1,
                hold_duration: Duration::from_millis(300),
            }
        );
        assert_eq!(
            sm.on_right_up(ScreenPoint::new(101, 101), t0 + Duration::from_millis(80)),
            StateAction::ReplayShortClick {
                down_at: ScreenPoint::new(100, 100),
                up_at: ScreenPoint::new(101, 101),
            }
        );
    }

    #[test]
    fn stationary_hold_triggers_and_swallows_up() {
        let mut sm = LongPressStateMachine::new(HookConfig::default());
        let t0 = Instant::now();
        let StateAction::SwallowAndArmTimer { generation, .. } =
            sm.on_right_down(ScreenPoint::new(200, 200), t0)
        else {
            panic!("expected SwallowAndArmTimer");
        };
        assert_eq!(
            sm.on_mouse_move(ScreenPoint::new(204, 203)),
            StateAction::PassThrough
        );
        // Stale generation must not trigger.
        assert_eq!(
            sm.on_timer_fired(generation + 99, t0 + Duration::from_millis(310)),
            None
        );
        assert_eq!(
            sm.on_timer_fired(generation, t0 + Duration::from_millis(310)),
            Some(StateAction::TriggerLongPress {
                position: ScreenPoint::new(200, 200),
            })
        );
        assert_eq!(
            sm.on_right_up(ScreenPoint::new(204, 203), t0 + Duration::from_millis(400)),
            StateAction::Swallow
        );
    }

    #[test]
    fn drag_beyond_threshold_replays_drag_start() {
        let mut sm = LongPressStateMachine::new(HookConfig::default());
        let t0 = Instant::now();
        sm.on_right_down(ScreenPoint::new(50, 50), t0);
        assert_eq!(
            sm.on_mouse_move(ScreenPoint::new(70, 50)),
            StateAction::ReplayDragStart {
                down_at: ScreenPoint::new(50, 50),
                current: ScreenPoint::new(70, 50),
            }
        );
        assert_eq!(
            sm.on_right_up(ScreenPoint::new(120, 50), t0 + Duration::from_millis(500)),
            StateAction::PassThrough
        );
    }
}
