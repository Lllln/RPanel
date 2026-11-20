/// 将面板放在光标后方。空间不足时翻转，最终限制在工作区内。
/// 调用方先确保工作区能容纳面板，所有参数须采用同一坐标系和单位。
pub fn axis_position(cursor: i32, start: i32, end: i32, size: i32, gap: i32) -> i32 {
    let after = cursor + gap;
    let preferred = if after <= end - size {
        after
    } else {
        cursor - gap - size
    };
    preferred.clamp(start, end - size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_a_gap_and_flips_at_the_far_edge() {
        assert_eq!(axis_position(100, 0, 1920, 168, 10), 110);
        assert_eq!(axis_position(1910, 0, 1920, 168, 10), 1732);
    }

    #[test]
    fn supports_negative_monitor_coordinates_and_taskbar_overlap() {
        assert_eq!(axis_position(-20, -1920, 0, 168, 10), -198);
        assert_eq!(axis_position(-1915, -1872, 0, 168, 10), -1872);
        assert_eq!(axis_position(1078, 0, 1040, 52, 10), 988);
    }
}
