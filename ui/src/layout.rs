//! Layout constants shared by the workspace chrome.
//!
//! These mirror values the official client expresses in CSS. They live outside
//! the wasm-only `web` module so the host test suite can pin them: a drifted
//! width is invisible in review but obvious on screen.

/// The shared content axis: transcript, dock cards, composer and takeover cards
/// all ride this width.
///
/// Official `ConversationRoot.module.css`:
/// `--dsh-chat-content-width: clamp(680px, calc(column * 0.64), 920px)`.
///
/// There is deliberately no extra edge cap. The official column already
/// excludes the sidebar, and the 680px floor holds at every window width;
/// subtracting additional edge budget shrank the transcript below that floor in
/// narrow windows (the default window is 600px wide).
pub fn conversation_axis(main_width: f32) -> f32 {
    (main_width * 0.64).clamp(680., 920.)
}

#[cfg(test)]
mod tests {
    use super::conversation_axis;

    #[test]
    fn content_axis_follows_the_official_clamp() {
        // Below the floor the official 680px minimum wins, even in the default
        // narrow window where the sidebar claims part of the width.
        assert_eq!(conversation_axis(0.), 680.);
        assert_eq!(conversation_axis(600.), 680.);
        assert_eq!(conversation_axis(1000.), 680.);
        // Inside the ramp the axis is 64% of the column.
        assert_eq!(conversation_axis(1100.), 704.);
        assert_eq!(conversation_axis(1250.), 800.);
        // Above the ceiling it saturates.
        assert_eq!(conversation_axis(1600.), 920.);
        assert_eq!(conversation_axis(4000.), 920.);
    }

    #[test]
    fn the_axis_never_dips_below_the_official_floor() {
        // A sweep over plausible column widths: the floor is the invariant that
        // an extra edge cap would break.
        for width in (0..=3000).step_by(7) {
            let axis = conversation_axis(width as f32);
            assert!(
                (680. ..=920.).contains(&axis),
                "axis {axis} out of range at column {width}"
            );
        }
    }
}
