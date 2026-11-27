use gpui_kit::{App, Bounds, DisplayId, Pixels, Point, Size, px, size};
use input_hook::ScreenPoint;

pub const PANEL_WIDTH: i32 = 680;
pub const PANEL_HEIGHT: i32 = 480;
pub const QUICK_MENU_WIDTH: i32 = 152;
pub const QUICK_MENU_HEIGHT: i32 = 48;
pub const QUICK_MENU_INSIGHT_WIDTH: i32 = 348;
pub const QUICK_MENU_INSIGHT_HEIGHT: i32 = 112;
pub const EDGE_MARGIN: i32 = 16;

/// Logical rectangle representing a monitor work area or floating panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

const CASCADE_STEP_PX: i32 = 28;
const CASCADE_WRAP: usize = 6;

/// Resolve the target display and clamped bounds for the floating JSON editor panel.
/// When `cursor` is `None` and `cascade_index > 0`, shifts the centered window gently so multiple
/// comparison panels do not stack directly on top of one another.
pub fn resolve_panel_placement(
    cursor: Option<ScreenPoint>,
    cascade_index: usize,
    cx: &App,
) -> (Option<DisplayId>, Bounds<Pixels>) {
    let (display_id, mut bounds) = resolve_window_placement(cursor, PANEL_WIDTH, PANEL_HEIGHT, 14, cx);
    if cascade_index > 0 {
        let step = ((cascade_index % CASCADE_WRAP) as i32) * CASCADE_STEP_PX;
        let target_display = display_id
            .and_then(|id| cx.displays().into_iter().find(|d| d.id() == id))
            .or_else(|| cx.primary_display());
        if let Some(display) = target_display {
            let b = display.bounds();
            let min_x = f32::from(b.origin.x).round() as i32 + EDGE_MARGIN;
            let min_y = f32::from(b.origin.y).round() as i32 + EDGE_MARGIN;
            let max_x = (f32::from(b.origin.x + b.size.width).round() as i32
                - PANEL_WIDTH
                - EDGE_MARGIN)
                .max(min_x);
            let max_y = (f32::from(b.origin.y + b.size.height).round() as i32
                - PANEL_HEIGHT
                - EDGE_MARGIN)
                .max(min_y);
            let nx = (f32::from(bounds.origin.x).round() as i32 + step).clamp(min_x, max_x);
            let ny = (f32::from(bounds.origin.y).round() as i32 + step).clamp(min_y, max_y);
            bounds.origin = Point::new(px(nx as f32), px(ny as f32));
        } else {
            bounds.origin = Point::new(
                bounds.origin.x + px(step as f32),
                bounds.origin.y + px(step as f32),
            );
        }
    }
    (display_id, bounds)
}

/// Resolve the target display and clamped bounds for the compact right-click quick menu.
pub fn resolve_quick_menu_placement(
    cursor: Option<ScreenPoint>,
    has_insight: bool,
    cx: &App,
) -> (Option<DisplayId>, Bounds<Pixels>) {
    let (width, height) = if has_insight {
        (QUICK_MENU_INSIGHT_WIDTH, QUICK_MENU_INSIGHT_HEIGHT)
    } else {
        (QUICK_MENU_WIDTH, QUICK_MENU_HEIGHT)
    };
    resolve_window_placement(cursor, width, height, 10, cx)
}

fn resolve_window_placement(
    cursor: Option<ScreenPoint>,
    width: i32,
    height: i32,
    cursor_offset: i32,
    cx: &App,
) -> (Option<DisplayId>, Bounds<Pixels>) {
    let win_size: Size<Pixels> = size(px(width as f32), px(height as f32));

    let Some(raw_cursor) = cursor else {
        let primary = cx.primary_display();
        let display_id = primary.as_ref().map(|d| d.id());
        let bounds = Bounds::centered(display_id, win_size, cx);
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
            let w = f32::from(b.size.width).round() as i32;
            let h = f32::from(b.size.height).round() as i32;
            logical_cursor.x >= left
                && logical_cursor.x < left + w
                && logical_cursor.y >= top
                && logical_cursor.y < top + h
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
            width,
            height,
            work_area,
            EDGE_MARGIN,
            cursor_offset,
        );
        let bounds = Bounds::new(Point::new(px(x as f32), px(y as f32)), win_size);
        return (Some(display.id()), bounds);
    }

    (None, Bounds::centered(None, win_size, cx))
}

/// Compute an in-bounds top-left coordinate for a floating window anchored near the cursor.
pub fn place_near_cursor(
    cursor_x: i32,
    cursor_y: i32,
    panel_width: i32,
    panel_height: i32,
    work_area: ScreenRect,
    margin: i32,
    offset: i32,
) -> (i32, i32) {
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
        let (x, y) = place_near_cursor(1900, 1060, 680, 480, work, 16, 14);
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
        let (x, y) = place_near_cursor(-1910, 50, 680, 480, work, 16, 14);
        assert!(x >= -1904 && x + 680 <= -16);
        assert!(y >= 16 && y + 480 <= 1064);
    }
}
