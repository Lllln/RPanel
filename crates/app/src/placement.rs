use gpui_kit::{App, Bounds, DisplayId, Pixels, Point, Size, px, size};
use input_hook::ScreenPoint;

pub const PANEL_WIDTH: i32 = 680;
pub const PANEL_HEIGHT: i32 = 480;
pub const EDGE_MARGIN: i32 = 16;

/// Logical rectangle representing a monitor work area or floating panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Resolve the target display and clamped floating panel bounds for an optional cursor position.
pub fn resolve_panel_placement(
    cursor: Option<ScreenPoint>,
    cx: &App,
) -> (Option<DisplayId>, Bounds<Pixels>) {
    let panel_size: Size<Pixels> = size(px(PANEL_WIDTH as f32), px(PANEL_HEIGHT as f32));

    let Some(raw_cursor) = cursor else {
        let primary = cx.primary_display();
        let display_id = primary.as_ref().map(|d| d.id());
        let bounds = Bounds::centered(display_id, panel_size, cx);
        return (display_id, bounds);
    };

    let logical_cursor = input_hook::to_logical_point(raw_cursor);
    let displays = cx.displays();

    let target_display = displays
        .into_iter()
        .find(|display| {
            let b = display.bounds();
            let left = f32::from(b.origin.x).round() as i32;
            let top = f32::from(b.origin.y).round() as i32;
            let width = f32::from(b.size.width).round() as i32;
            let height = f32::from(b.size.height).round() as i32;
            logical_cursor.x >= left
                && logical_cursor.x < left + width
                && logical_cursor.y >= top
                && logical_cursor.y < top + height
        })
        .or_else(|| cx.primary_display());

    if let Some(display) = target_display {
        let b = display.bounds();
        let work_area = ScreenRect {
            x: f32::from(b.origin.x).round() as i32,
            y: f32::from(b.origin.y).round() as i32,
            width: f32::from(b.size.width).round() as i32,
            height: f32::from(b.size.height).round() as i32,
        };
        let (x, y) = place_near_cursor(
            logical_cursor.x,
            logical_cursor.y,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            work_area,
            EDGE_MARGIN,
        );
        let bounds = Bounds::new(Point::new(px(x as f32), px(y as f32)), panel_size);
        return (Some(display.id()), bounds);
    }

    (None, Bounds::centered(None, panel_size, cx))
}

/// Compute an in-bounds top-left coordinate for a floating panel anchored near the cursor.
pub fn place_near_cursor(
    cursor_x: i32,
    cursor_y: i32,
    panel_width: i32,
    panel_height: i32,
    work_area: ScreenRect,
    margin: i32,
) -> (i32, i32) {
    let offset = 14;
    let max_x = (work_area.x + work_area.width - panel_width - margin).max(work_area.x + margin);
    let max_y = (work_area.y + work_area.height - panel_height - margin).max(work_area.y + margin);

    let preferred_x = if cursor_x + offset + panel_width <= work_area.x + work_area.width - margin {
        cursor_x + offset
    } else {
        cursor_x - offset - panel_width
    };

    let preferred_y = if cursor_y + offset + panel_height <= work_area.y + work_area.height - margin
    {
        cursor_y + offset
    } else {
        cursor_y - offset - panel_height
    };

    let x = preferred_x.clamp(work_area.x + margin, max_x);
    let y = preferred_y.clamp(work_area.y + margin, max_y);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_panel_within_right_bottom_edge() {
        let work = ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let (x, y) = place_near_cursor(1900, 1060, 680, 480, work, 16);
        assert!(x >= 16 && x + 680 <= 1904);
        assert!(y >= 16 && y + 480 <= 1064);
    }

    #[test]
    fn supports_negative_secondary_display_coordinates() {
        let work = ScreenRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let (x, y) = place_near_cursor(-1910, 50, 680, 480, work, 16);
        assert!(x >= -1904 && x + 680 <= -16);
        assert!(y >= 16 && y + 480 <= 1064);
    }
}
