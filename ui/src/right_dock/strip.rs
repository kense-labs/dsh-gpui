//! Scroll and keyboard geometry shared by the dock tab strips.

use super::dock_easing;

pub(crate) const STRIP_FADE: f32 = 24.;

pub(crate) fn strip_focus_index(key: &str, index: usize, count: usize) -> Option<usize> {
    if index >= count {
        return None;
    }
    match key {
        "left" => Some(if index == 0 { count - 1 } else { index - 1 }),
        "right" => Some((index + 1) % count),
        "home" => Some(0),
        "end" => Some(count - 1),
        _ => None,
    }
}

/// The smallest scroll that brings a clipped chip back, clearing the edge fade when possible.
pub(crate) fn strip_reveal_offset(
    offset: f32,
    maximum: f32,
    viewport: f32,
    left: f32,
    right: f32,
) -> f32 {
    let target = if left < offset {
        left - STRIP_FADE
    } else if right > offset + viewport {
        right - viewport + STRIP_FADE
    } else {
        offset
    };
    target.clamp(0., maximum)
}

#[derive(Clone, Copy)]
pub(crate) struct StripGlide {
    pub(crate) target: f32,
    from: f32,
    start_ms: f64,
    duration_ms: f64,
}

impl StripGlide {
    pub(crate) fn new(from: f32, target: f32, start_ms: f64) -> Self {
        // Chromium's desktop programmatic scroll curve, used by the official strip's scrollLeft.
        let travel = f64::from((target - from).abs());
        let duration_ms = (travel.sqrt() * 1000. / 60.).min(1500.);
        Self {
            from,
            target,
            start_ms,
            duration_ms,
        }
    }

    pub(crate) fn sample(self, now: f64) -> (f32, bool) {
        let time = now - self.start_ms;
        if time >= self.duration_ms {
            return (self.target, false);
        }
        let elapsed = (time / self.duration_ms).max(0.);
        let progress = dock_easing(elapsed, 0.4, 0.) as f32;
        (self.from + (self.target - self.from) * progress, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_navigation_wraps_and_jumps_to_strip_edges() {
        assert_eq!(strip_focus_index("left", 0, 4), Some(3));
        assert_eq!(strip_focus_index("right", 3, 4), Some(0));
        assert_eq!(strip_focus_index("home", 2, 4), Some(0));
        assert_eq!(strip_focus_index("end", 2, 4), Some(3));
        assert_eq!(strip_focus_index("enter", 2, 4), None);
        assert_eq!(strip_focus_index("left", 0, 0), None);
    }

    #[test]
    fn chip_reveal_clears_the_fade_and_preserves_manual_scroll_when_visible() {
        assert_eq!(strip_reveal_offset(96., 300., 160., 110., 230.), 96.);
        assert_eq!(strip_reveal_offset(96., 300., 160., 50., 160.), 26.);
        assert_eq!(strip_reveal_offset(96., 300., 160., 220., 320.), 184.);
        assert_eq!(strip_reveal_offset(96., 300., 160., 0., 100.), 0.);
        assert_eq!(strip_reveal_offset(96., 300., 160., 360., 460.), 300.);
    }

    #[test]
    fn glide_uses_the_distance_based_duration_and_lands_at_the_scroll_offset() {
        let glide = StripGlide::new(10., 154., 50.);
        assert_eq!(glide.sample(50.), (10., true));
        let (midpoint, running) = glide.sample(150.);
        assert!(running);
        assert!((midpoint - 134.059).abs() < 0.01);
        assert_eq!(glide.sample(250.), (154., false));
        assert_eq!(
            StripGlide::new(0., 10000., 0.).sample(1500.),
            (10000., false)
        );
    }
}
