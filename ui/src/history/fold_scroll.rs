//! Scroll landing for a batch of transcript folds. Geometry comes from the layout frame.

#[derive(Clone, Copy)]
pub(crate) struct FoldGeometry {
    pub(crate) top: f32,
    pub(crate) height: f32,
    pub(crate) content: f32,
    pub(crate) remaining: f32,
    pub(crate) progress: f32,
}

impl FoldGeometry {
    fn floor(self) -> f32 {
        (self.content + self.remaining - self.height).max(0.)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Intent {
    Tail,
    Reading,
    Interrupted,
}

pub(crate) struct FoldLanding {
    intent: Intent,
    base: f32,
    target: f32,
    segment_top: f32,
    segment_progress: f32,
    previous_progress: f32,
    written: f32,
    origin: f32,
}

impl FoldLanding {
    pub(crate) fn new(geometry: FoldGeometry, covered: f32, follow: bool) -> Self {
        let base = (geometry.top - covered).max(0.);
        Self {
            intent: if follow {
                Intent::Tail
            } else {
                Intent::Reading
            },
            base,
            target: if follow {
                base.max(geometry.floor())
            } else {
                base
            },
            segment_top: geometry.top,
            segment_progress: 0.,
            previous_progress: 0.,
            written: geometry.top,
            origin: geometry.top,
        }
    }

    /// Move on the box clock. Reserve only the room the sampled position needs.
    pub(crate) fn sample(
        &mut self,
        geometry: FoldGeometry,
        follows_tail: bool,
        request_tail: bool,
    ) -> (f32, f32) {
        if request_tail {
            self.intent = Intent::Tail;
            self.segment_top = geometry.top;
            self.segment_progress = self.previous_progress;
        } else if self.intent == Intent::Tail && !follows_tail {
            self.intent = Intent::Interrupted;
        }
        if self.intent == Intent::Reading {
            let drift = geometry.top - self.written;
            if drift.abs() > 0.5 {
                self.base = (self.base + drift).max(0.);
                self.segment_top += drift;
                self.target = self.base;
            }
        }
        let target = if self.intent == Intent::Tail {
            self.base.max(geometry.floor())
        } else {
            self.base
        };
        if (target - self.target).abs() > 0.5 {
            self.segment_top = geometry.top;
            self.segment_progress = self.previous_progress;
        }
        self.target = target;
        let progress = if self.segment_progress >= 1. {
            1.
        } else {
            ((geometry.progress - self.segment_progress) / (1. - self.segment_progress))
                .clamp(0., 1.)
        };
        let top = if self.intent == Intent::Interrupted {
            geometry.top
        } else {
            self.segment_top + (self.target - self.segment_top) * progress
        };
        let top = top.max(0.);
        let room = if top > 0. {
            (top + geometry.height - geometry.content).max(0.)
        } else {
            0.
        };
        let final_room = if self.intent != Intent::Interrupted && self.target > 0. {
            (self.target + geometry.height - geometry.content - geometry.remaining).max(0.)
        } else {
            0.
        };
        self.written = top;
        self.previous_progress = geometry.progress;
        (top, room.max(final_room))
    }

    pub(crate) fn displacement(&self, top: f32) -> f32 {
        top - self.origin
    }

    pub(crate) fn travels_forward(&self) -> bool {
        self.intent == Intent::Tail && self.target > self.segment_top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_and_scroll_land_on_the_same_progress() {
        // The official E/E, T/B, B/B and short-content geometry fixtures.
        for (earlier, steps, body, top, end, room) in [
            (2400., 1000., 400., 800., 2080., 0.),
            (500., 1200., 400., 1200., 500., 320.),
            (2400., 1600., 1000., 4200., 2680., 0.),
            (100., 300., 100., 0., 0., 0.),
        ] {
            let start = FoldGeometry {
                top,
                height: 800.,
                content: earlier + steps + body + 80.,
                remaining: -steps,
                progress: 0.,
            };
            let mut fold = FoldLanding::new(start, (top - earlier).clamp(0., steps), true);
            let (middle, _) = fold.sample(
                FoldGeometry {
                    content: start.content - steps / 2.,
                    remaining: -steps / 2.,
                    progress: 0.5,
                    ..start
                },
                true,
                false,
            );
            assert_eq!(middle, (top + end) / 2.);
            assert_eq!(
                fold.sample(
                    FoldGeometry {
                        top: middle,
                        content: start.content - steps,
                        remaining: 0.,
                        progress: 1.,
                        ..start
                    },
                    true,
                    false
                ),
                (end, room)
            );
        }
    }

    #[test]
    fn upward_reading_cancels_a_tail_landing_and_keeps_required_room() {
        let start = FoldGeometry {
            top: 1200.,
            height: 800.,
            content: 2180.,
            remaining: -1200.,
            progress: 0.,
        };
        let mut fold = FoldLanding::new(start, 700., true);
        let (middle, _) = fold.sample(
            FoldGeometry {
                content: 1580.,
                remaining: -600.,
                progress: 0.5,
                ..start
            },
            true,
            false,
        );
        let reading = middle - 20.;
        assert_eq!(
            fold.sample(
                FoldGeometry {
                    top: reading,
                    content: 980.,
                    remaining: 0.,
                    progress: 1.,
                    ..start
                },
                false,
                false
            ),
            (reading, reading - 180.)
        );
    }

    #[test]
    fn streaming_growth_retargets_without_jumping_the_current_frame() {
        let start = FoldGeometry {
            top: 4200.,
            height: 800.,
            content: 5080.,
            remaining: -1600.,
            progress: 0.,
        };
        let mut fold = FoldLanding::new(start, 1600., true);
        let halfway = FoldGeometry {
            content: 4280.,
            remaining: -800.,
            progress: 0.5,
            ..start
        };
        let (middle, _) = fold.sample(halfway, true, false);
        let after_growth = FoldGeometry {
            top: middle,
            content: halfway.content + 320.,
            ..halfway
        };
        assert_eq!(fold.sample(after_growth, true, false).0, middle);
        assert_eq!(
            fold.sample(
                FoldGeometry {
                    content: 3800.,
                    remaining: 0.,
                    progress: 1.,
                    ..after_growth
                },
                true,
                false
            ),
            (3000., 0.)
        );
    }
}
