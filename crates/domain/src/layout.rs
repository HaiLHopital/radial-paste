use std::f64::consts::TAU;
use std::ops::{Div, Sub};

use serde::{Deserialize, Serialize};

/// A position or offset in physical screen pixels, `y` pointing down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;

    fn sub(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x - other.x, self.y - other.y)
    }
}

impl Div<f64> for Vec2 {
    type Output = Vec2;

    /// Scales an offset down, e.g. from physical pixels to the `Layout`'s logical ones.
    fn div(self, scale: f64) -> Vec2 {
        Vec2::new(self.x / scale, self.y / scale)
    }
}

/// The size of the ring, in logical (96-DPI) pixels from its centre. On a higher-DPI monitor
/// the adapter multiplies by the monitor's scale, and the core divides offsets by it.
///
/// Wedge 0 is centred at 12 o'clock and the rest follow clockwise, each `360° / count` wide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Within this distance of the centre nothing is picked.
    pub dead_zone: f64,
    /// The ring's outer edge. A click beyond it closes the menu; a flick beyond it still picks.
    pub radius: f64,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            dead_zone: 24.0,
            radius: 140.0,
        }
    }
}

impl Layout {
    pub fn is_valid(&self) -> bool {
        self.dead_zone.is_finite()
            && self.radius.is_finite()
            && self.dead_zone >= 0.0
            && self.radius > self.dead_zone
    }

    /// The wedge, out of `count`, that `offset` from the centre points into. `None` in the dead
    /// zone or when there are no wedges. Distance beyond the radius does not matter.
    pub fn pick(&self, count: usize, offset: Vec2) -> Option<usize> {
        if count == 0 || offset.length() <= self.dead_zone {
            return None;
        }
        let step = TAU / count as f64;
        // Clockwise from 12 o'clock, in [0, TAU).
        let angle = offset.x.atan2(-offset.y).rem_euclid(TAU);
        Some(((angle + step / 2.0) / step) as usize % count)
    }

    /// Whether `offset` from the centre is on the menu.
    pub fn contains(&self, offset: Vec2) -> bool {
        offset.length() <= self.radius
    }

    /// Where to centre wedge `index`'s label, as an offset from the menu's centre.
    pub fn label_offset(&self, count: usize, index: usize) -> Vec2 {
        let angle = TAU * index as f64 / count.max(1) as f64;
        let distance = (self.dead_zone + self.radius) / 2.0;
        Vec2::new(distance * angle.sin(), -distance * angle.cos())
    }
}

/// A rectangle in physical screen pixels, `right` and `bottom` excluded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// Moves `centre` inward so that a ring of `radius` around it stays inside `area`: near a
/// screen edge the menu would otherwise be cut off. `radius` is in the same units as the
/// rectangle, so the adapter scales the logical `Layout::radius` first. If the area is smaller
/// than the ring, the right and bottom clamps win; `place` never panics.
pub fn place(centre: Vec2, area: Rect, radius: f64) -> Vec2 {
    Vec2::new(
        clamp_to(centre.x, area.left + radius, area.right - radius),
        clamp_to(centre.y, area.top + radius, area.bottom - radius),
    )
}

fn clamp_to(value: f64, min: f64, max: f64) -> f64 {
    if min > max {
        max
    } else {
        value.clamp(min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: Layout = Layout {
        dead_zone: 20.0,
        radius: 100.0,
    };

    #[test]
    fn four_wedges_point_up_right_down_left() {
        assert_eq!(LAYOUT.pick(4, Vec2::new(0.0, -50.0)), Some(0));
        assert_eq!(LAYOUT.pick(4, Vec2::new(50.0, 0.0)), Some(1));
        assert_eq!(LAYOUT.pick(4, Vec2::new(0.0, 50.0)), Some(2));
        assert_eq!(LAYOUT.pick(4, Vec2::new(-50.0, 0.0)), Some(3));
    }

    #[test]
    fn wedge_zero_straddles_twelve_o_clock() {
        assert_eq!(LAYOUT.pick(4, Vec2::new(-30.0, -50.0)), Some(0));
        assert_eq!(LAYOUT.pick(4, Vec2::new(30.0, -50.0)), Some(0));
    }

    #[test]
    fn eight_wedges_include_the_diagonals() {
        assert_eq!(LAYOUT.pick(8, Vec2::new(40.0, -40.0)), Some(1));
        assert_eq!(LAYOUT.pick(8, Vec2::new(-40.0, -40.0)), Some(7));
    }

    #[test]
    fn dead_zone_and_empty_menu_pick_nothing() {
        assert_eq!(LAYOUT.pick(4, Vec2::new(10.0, 10.0)), None);
        assert_eq!(LAYOUT.pick(0, Vec2::new(0.0, -50.0)), None);
    }

    #[test]
    fn one_wedge_takes_every_direction() {
        assert_eq!(LAYOUT.pick(1, Vec2::new(0.0, 50.0)), Some(0));
        assert_eq!(LAYOUT.pick(1, Vec2::new(-50.0, 0.0)), Some(0));
    }

    #[test]
    fn flicks_past_the_radius_still_pick() {
        assert_eq!(LAYOUT.pick(4, Vec2::new(500.0, 0.0)), Some(1));
        assert!(!LAYOUT.contains(Vec2::new(500.0, 0.0)));
    }

    #[test]
    fn labels_sit_mid_ring_in_their_wedge() {
        let offset = LAYOUT.label_offset(4, 1);
        assert!((offset.x - 60.0).abs() < 1e-9 && offset.y.abs() < 1e-9);
        assert_eq!(LAYOUT.pick(4, offset), Some(1));
    }

    #[test]
    fn layout_needs_a_ring_outside_the_dead_zone() {
        assert!(LAYOUT.is_valid());
        assert!(
            !Layout {
                dead_zone: 50.0,
                radius: 50.0
            }
            .is_valid()
        );
        assert!(
            !Layout {
                dead_zone: -1.0,
                radius: 50.0
            }
            .is_valid()
        );
        assert!(
            !Layout {
                dead_zone: 0.0,
                radius: f64::NAN
            }
            .is_valid()
        );
    }

    #[test]
    fn dividing_scales_both_axes() {
        let offset = Vec2::new(10.0, -30.0) / 2.0;
        assert_eq!(offset, Vec2::new(5.0, -15.0));
    }

    fn screen() -> Rect {
        Rect {
            left: 0.0,
            top: 0.0,
            right: 1000.0,
            bottom: 800.0,
        }
    }

    #[test]
    fn place_keeps_a_centre_that_already_fits() {
        let centre = Vec2::new(500.0, 400.0);
        assert_eq!(place(centre, screen(), 140.0), centre);
    }

    #[test]
    fn place_moves_the_centre_off_the_edges() {
        assert_eq!(
            place(Vec2::new(30.0, 400.0), screen(), 140.0),
            Vec2::new(140.0, 400.0)
        );
        assert_eq!(
            place(Vec2::new(500.0, 20.0), screen(), 140.0),
            Vec2::new(500.0, 140.0)
        );
        assert_eq!(
            place(Vec2::new(990.0, 790.0), screen(), 140.0),
            Vec2::new(860.0, 660.0)
        );
    }

    #[test]
    fn place_survives_an_area_smaller_than_the_ring() {
        let tiny = Rect {
            left: 0.0,
            top: 0.0,
            right: 100.0,
            bottom: 100.0,
        };
        assert_eq!(
            place(Vec2::new(50.0, 50.0), tiny, 140.0),
            Vec2::new(-40.0, -40.0)
        );
    }
}
