//! Floating panel geometry in logical pixels.

use serde::{Deserialize, Serialize};

const RESISTANCE: f32 = 120.;
const MIN_WIDTH: f32 = 220.;
const MIN_HEIGHT: f32 = 140.;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct FloatRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl FloatRect {
    pub(crate) fn at_drop(x: f32, y: f32) -> Self {
        Self {
            x: (x - 60.).max(0.),
            y: (y - 14.).max(0.),
            width: 380.,
            height: 300.,
        }
    }

    pub(super) fn valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.
            && self.height > 0.
    }

    pub(crate) fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FloatViewport {
    pub(crate) top: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl FloatViewport {
    pub(crate) fn project(self, rect: FloatRect) -> FloatRect {
        let top = self.top.min(self.height);
        let width = rect.width.min(self.width);
        let height = rect.height.min(self.height - top);
        FloatRect {
            x: rect.x.clamp(0., self.width - width),
            y: rect.y.clamp(top, self.height - height),
            width,
            height,
        }
    }

    pub(crate) fn resisted(self, rect: FloatRect) -> FloatRect {
        let legal = self.project(rect);
        let resist = |overflow: f32| RESISTANCE * overflow / (RESISTANCE + overflow.abs());
        FloatRect {
            x: legal.x + resist(rect.x - legal.x),
            y: legal.y + resist(rect.y - legal.y),
            ..legal
        }
    }

    pub(crate) fn unresisted(self, rect: FloatRect) -> FloatRect {
        let legal = self.project(rect);
        let unresist = |offset: f32| {
            let distance = offset.abs().min(RESISTANCE * 0.99);
            offset.signum() * RESISTANCE * distance / (RESISTANCE - distance)
        };
        FloatRect {
            x: legal.x + unresist(rect.x - legal.x),
            y: legal.y + unresist(rect.y - legal.y),
            ..legal
        }
    }

    pub(crate) fn bounded_resize(self, rect: FloatRect) -> FloatRect {
        FloatRect {
            width: rect.width.min(self.width - rect.x).max(MIN_WIDTH),
            height: rect.height.min(self.height - rect.y).max(MIN_HEIGHT),
            ..rect
        }
    }
}

const OMEGA: f32 = std::f32::consts::TAU / 0.32;
const DAMPING: f32 = 0.72;

#[derive(Clone, Copy)]
pub(crate) struct SpringAxis {
    pub(crate) position: f32,
    velocity: f32,
}

impl SpringAxis {
    pub(crate) fn new(position: f32, target: f32, velocity: f32) -> Self {
        let offset = position - target;
        let mut velocity = velocity.clamp(-1500., 1500.);
        if offset == 0. {
            velocity = 0.;
        } else if velocity.signum() == offset.signum() {
            velocity = offset.signum()
                * velocity
                    .abs()
                    .min(OMEGA * (RESISTANCE - offset.abs()).max(0.));
        }
        Self { position, velocity }
    }

    pub(crate) fn step(&mut self, target: f32, elapsed_ms: f64) -> bool {
        let seconds = elapsed_ms.min(64.) as f32 / 1000.;
        let steps = (seconds * 480.).ceil().max(1.) as usize;
        let step = seconds / steps as f32;
        for _ in 0..steps {
            self.velocity += (-OMEGA * OMEGA * (self.position - target)
                - 2. * DAMPING * OMEGA * self.velocity)
                * step;
            self.position += self.velocity * step;
        }
        (self.position - target).abs() <= 1. && self.velocity.abs() <= 60.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_geometry_clamps_to_viewport_and_preserves_the_grab_offset() {
        let area = FloatViewport {
            top: 20.,
            width: 660.,
            height: 440.,
        };
        let dropped = FloatRect::at_drop(600., 50.);
        assert_eq!(
            dropped,
            FloatRect {
                x: 540.,
                y: 36.,
                width: 380.,
                height: 300.
            }
        );
        assert_eq!(area.project(dropped), FloatRect { x: 280., ..dropped });
        assert_eq!(area.project(FloatRect::at_drop(0., 0.)).y, 20.);
        let shrunk = FloatViewport {
            width: 200.,
            height: 130.,
            ..area
        }
        .project(dropped);
        assert_eq!(
            shrunk,
            FloatRect {
                x: 0.,
                y: 20.,
                width: 200.,
                height: 110.
            }
        );
    }

    #[test]
    fn resize_preserves_its_origin_and_overlap_requires_shared_area() {
        let area = FloatViewport {
            top: 0.,
            width: 600.,
            height: 400.,
        };
        let request = FloatRect {
            x: 100.,
            y: 80.,
            width: 80.,
            height: 500.,
        };
        let sized = area.bounded_resize(request);
        assert_eq!(
            sized,
            FloatRect {
                width: 220.,
                height: 320.,
                ..request
            }
        );
        let adjacent = FloatRect { x: 320., ..sized };
        assert!(!sized.intersects(adjacent));
        assert!(sized.intersects(FloatRect {
            x: 319.,
            ..adjacent
        }));
    }

    #[test]
    fn resisted_drag_can_be_picked_up_in_place_and_springs_back() {
        let area = FloatViewport {
            top: 0.,
            width: 1000.,
            height: 600.,
        };
        let desired = FloatRect {
            x: -120.,
            ..FloatRect::at_drop(0., 100.)
        };
        let drawn = area.resisted(desired);
        assert_eq!(drawn.x, -60.);
        assert_eq!(area.unresisted(drawn), desired);
        let mut spring = SpringAxis::new(drawn.x, 0., -300.);
        let mut frames = 0;
        while !spring.step(0., 16.) {
            frames += 1;
            assert!(spring.position.abs() < RESISTANCE);
            assert!(frames < 56);
        }
        assert!(frames > 0);
    }
}
