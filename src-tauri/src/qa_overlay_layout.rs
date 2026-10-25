//! Physical desktop placement shared with the recording capsule's visible bottom.
//! Keep monitor origins and DPI conversion outside this pure geometry operation.

#[derive(Clone, Copy, Debug)]
pub(crate) struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn anchored_question_origin(
    monitor: PhysicalRect,
    capsule: PhysicalRect,
    question_size: (u32, u32),
    capsule_bottom_inset: u32,
) -> (i32, i32) {
    let x = i64::from(capsule.x)
        + (i64::from(capsule.width) - i64::from(question_size.0)) / 2;
    let y = i64::from(capsule.y) + i64::from(capsule.height)
        - i64::from(capsule_bottom_inset) - i64::from(question_size.1);
    let min_x = i64::from(monitor.x);
    let min_y = i64::from(monitor.y);
    let max_x = (min_x + i64::from(monitor.width) - i64::from(question_size.0)).max(min_x);
    let max_y = (min_y + i64::from(monitor.height) - i64::from(question_size.1)).max(min_y);
    (x.clamp(min_x, max_x) as i32, y.clamp(min_y, max_y) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_replaces_the_visible_capsule_area_from_the_same_bottom() {
        let monitor = PhysicalRect { x: 0, y: 0, width: 1920, height: 1080 };
        let capsule = PhysicalRect { x: 1592, y: 899, width: 304, height: 105 };
        let (x, y) = anchored_question_origin(monitor, capsule, (300, 176), 12);
        assert_eq!((x, y), (1594, 816));
        assert_eq!(y + 176, capsule.y + capsule.height as i32 - 12);
    }

    #[test]
    fn moved_capsule_and_negative_monitor_keep_physical_coordinates() {
        let monitor = PhysicalRect { x: -2560, y: -200, width: 2560, height: 1440 };
        let capsule = PhysicalRect { x: -1800, y: 800, width: 380, height: 131 };
        assert_eq!(anchored_question_origin(monitor, capsule, (375, 220), 15), (-1798, 696));
    }

    #[test]
    fn tall_card_at_a_top_edge_is_clamped_instead_of_lost() {
        let monitor = PhysicalRect { x: -1280, y: 100, width: 1280, height: 1024 };
        let capsule = PhysicalRect { x: -1280, y: 100, width: 304, height: 105 };
        assert_eq!(anchored_question_origin(monitor, capsule, (400, 240), 12), (-1280, 100));
    }

    #[test]
    fn oversized_window_does_not_create_an_invalid_clamp_range() {
        let monitor = PhysicalRect { x: 100, y: 200, width: 200, height: 100 };
        let capsule = PhysicalRect { x: 100, y: 200, width: 304, height: 105 };
        assert_eq!(anchored_question_origin(monitor, capsule, (400, 240), 12), (100, 200));
    }
}
