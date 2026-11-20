pub const DEFAULT_HOLD_MS: u32 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy)]
struct Press {
    at_ms: u32,
    origin: Point,
    tolerance_px: u32,
}

pub struct RightHold {
    threshold_ms: u32,
    press: Option<Press>,
}

impl RightHold {
    pub fn new(threshold_ms: u32) -> Self {
        Self {
            threshold_ms,
            press: None,
        }
    }

    pub fn press(&mut self, at_ms: u32, origin: Point, tolerance_px: u32) {
        self.press = Some(Press {
            at_ms,
            origin,
            tolerance_px,
        });
    }

    pub fn move_to(&mut self, point: Point) {
        if let Some(press) = self.press
            && (press.origin.x.abs_diff(point.x) > press.tolerance_px
                || press.origin.y.abs_diff(point.y) > press.tolerance_px)
        {
            // 拖出容差后即使移回原处，也不能恢复同一次手势。
            self.cancel();
        }
    }

    pub fn release(&mut self, at_ms: u32, point: Point) -> bool {
        self.move_to(point);
        self.press
            .take()
            .is_some_and(|press| at_ms.wrapping_sub(press.at_ms) >= self.threshold_ms)
    }

    pub fn cancel(&mut self) {
        self.press = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: Point = Point { x: -200, y: 100 };

    #[test]
    fn threshold_is_inclusive_and_each_press_can_trigger_only_once() {
        let mut hold = RightHold::new(DEFAULT_HOLD_MS);
        hold.press(10, ORIGIN, 8);
        assert!(!hold.release(509, ORIGIN));
        hold.press(1_000, ORIGIN, 8);
        assert!(hold.release(1_500, ORIGIN));
        assert!(!hold.release(1_501, ORIGIN));
    }

    #[test]
    fn dragging_out_and_back_cancels_the_gesture() {
        let mut hold = RightHold::new(DEFAULT_HOLD_MS);
        hold.press(10, ORIGIN, 8);
        hold.move_to(Point { x: -209, y: 100 });
        hold.move_to(ORIGIN);
        assert!(!hold.release(600, ORIGIN));
    }

    #[test]
    fn release_also_checks_distance_when_move_events_are_missing() {
        let mut hold = RightHold::new(DEFAULT_HOLD_MS);
        hold.press(10, ORIGIN, 8);
        assert!(!hold.release(600, Point { x: -200, y: 109 }));
    }

    #[test]
    fn cancellation_and_a_new_press_discard_old_state() {
        let mut hold = RightHold::new(DEFAULT_HOLD_MS);
        hold.press(10, ORIGIN, 8);
        hold.cancel();
        assert!(!hold.release(800, ORIGIN));
        hold.press(10, ORIGIN, 8);
        hold.press(1_000, ORIGIN, 8);
        assert!(!hold.release(1_001, ORIGIN));
    }

    #[test]
    fn windows_timestamp_wraparound_preserves_elapsed_time() {
        let mut hold = RightHold::new(300);
        hold.press(u32::MAX - 199, ORIGIN, 8);
        assert!(hold.release(100, ORIGIN));
    }
}
