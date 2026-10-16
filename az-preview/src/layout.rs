// SPDX-License-Identifier: GPL-3.0-only
//! Where each control sits on the panel, generated from layout/xdj-az.tsv.
//!
//! `u`/`v` are the control's center as fractions of the panel's width and
//! height; `hw`/`hh` are half its width and height, both as fractions of the
//! panel's width (so a circle has `hw == hh` = its radius).

/// A control's place, size and shape on the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub u: f32,
    pub v: f32,
    pub hw: f32,
    pub hh: f32,
    /// Round (a circle of radius `hw`); otherwise a rectangle, or `look.path`.
    pub round: bool,
    pub look: &'static Look,
}

/// How a control is drawn, measured on the same drawing as its place. Lengths
/// are fractions of the panel width; angles are degrees clockwise from 3
/// o'clock (screen coordinates, so 270 is straight up).
#[derive(Debug, PartialEq)]
pub struct Look {
    /// Concentric outlines, center out.
    pub rings: &'static [f32],
    /// A filled disc of this radius at the center (0: none).
    pub fill: f32,
    /// A knurled band: (grip count, radius of the grips).
    pub grips: (u16, f32),
    /// A scalloped skirt: (lobes, inner radius, outer radius).
    pub scallop: (u16, f32, f32),
    /// Scale dots printed around a knob: (radius, angle of each).
    pub dots: (f32, &'static [f32]),
    /// A printed arc around a knob: (radius, width, (start, end) of each part).
    pub arc: (f32, f32, &'static [(f32, f32)]),
    /// The corner radius of a rectangular control.
    pub corner: f32,
    /// A level meter's segments along its long side: (count, length of each).
    /// The first and last fill the ends; the rest are evenly spaced between.
    pub segs: (u16, f32),
    /// The outline, relative to the center, when it is neither a circle nor a
    /// rounded rectangle.
    pub path: &'static [(f32, f32)],
    /// The jog wheel's display and grip ring.
    pub jog: Option<JogLook>,
}

/// The jog's measured parts (lengths: fractions of the panel width).
#[derive(Debug, PartialEq)]
pub struct JogLook {
    /// The jog display: the radius of its image, and of its black surround.
    pub hub: (f32, f32),
    /// The grip ring's ovals: count (evenly spaced from 0 degrees), radius,
    /// along-ring and across-ring size.
    pub ovals: (u16, f32, f32, f32),
    /// The grip ring's small circles: size, and (angle after each oval, radius)
    /// of each.
    pub small: f32,
    pub smalls: &'static [(f32, f32)],
}

impl Geom {
    /// The same control on deck `d` (0 = left, 1 = right). The right deck is
    /// the left one shifted by [`DECK_R_DX`].
    pub fn on_deck(self, d: usize) -> Geom {
        Geom {
            u: self.u + if d == 0 { 0.0 } else { DECK_R_DX },
            ..self
        }
    }

    /// Whether a point on the panel (u, v) is on the control, by its real shape.
    pub fn contains(self, u: f32, v: f32) -> bool {
        // Offsets in fractions of the panel width on both axes.
        let (dx, dy) = (u - self.u, (v - self.v) / crate::panel::ASPECT);
        if !self.look.path.is_empty() {
            return inside_polygon(self.look.path, dx, dy);
        }
        if self.round {
            return dx * dx + dy * dy <= self.hw * self.hw;
        }
        let r = self.look.corner.min(self.hw).min(self.hh);
        let (qx, qy) = (dx.abs() - (self.hw - r), dy.abs() - (self.hh - r));
        if qx > r || qy > r {
            return false;
        }
        qx <= 0.0 || qy <= 0.0 || qx * qx + qy * qy <= r * r
    }

    /// Top and bottom of a vertical fader's travel (`v` of the cap's center).
    pub fn travel_v(self) -> (f32, f32) {
        let half = self.hh * crate::panel::ASPECT;
        (self.v - half, self.v + half)
    }

    /// Left and right of a horizontal fader's travel (`u` of the cap's center).
    pub fn travel_u(self) -> (f32, f32) {
        (self.u - self.hw, self.u + self.hw)
    }
}

/// Even-odd rule: whether (x, y) is inside the closed polygon `pts`.
fn inside_polygon(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for (i, &(xi, yi)) in pts.iter().enumerate() {
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

include!(concat!(env!("OUT_DIR"), "/layout.rs"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{DECK_ERP, DECK_TRANSPORT, GLOBAL_ERP, PANEL_BUTTONS};

    /// Panel pixels of a control's outline, for comparing rects.
    fn rect(g: Geom) -> (f32, f32, f32, f32) {
        let a = crate::panel::ASPECT;
        (g.u - g.hw, g.v - g.hh * a, g.u + g.hw, g.v + g.hh * a)
    }

    fn overlap(a: Geom, b: Geom) -> bool {
        let (a0, a1, a2, a3) = rect(a);
        let (b0, b1, b2, b3) = rect(b);
        a0 < b2 && b0 < a2 && a1 < b3 && b1 < a3
    }

    #[test]
    fn the_screen_keeps_its_aspect() {
        // A tap maps to a guest pixel by its fraction of this rect, so it must be
        // exactly 1280 x 800 in shape.
        let (w, h) = (TOP_LCD.hw * 2.0, TOP_LCD.hh * 2.0);
        assert!((w / h - 1.6).abs() < 0.002, "{}", w / h);
    }

    #[test]
    fn every_control_is_on_the_panel() {
        let all = DECK_ERP
            .iter()
            .chain(DECK_TRANSPORT)
            .flat_map(|b| [b.g.on_deck(0), b.g.on_deck(1)])
            .chain(PANEL_BUTTONS.iter().chain(GLOBAL_ERP).map(|b| b.g));
        for g in all {
            let (l, t, r, b) = rect(g);
            assert!(l > 0.0 && t > 0.0 && r < 1.0 && b < 1.0, "{g:?}");
        }
    }

    #[test]
    fn hit_areas_never_overlap() {
        // A click lands on exactly one button.
        let mut hits: Vec<Geom> = Vec::new();
        for d in 0..2 {
            hits.extend(
                DECK_ERP
                    .iter()
                    .chain(DECK_TRANSPORT)
                    .map(|b| b.g.on_deck(d)),
            );
        }
        hits.extend(PANEL_BUTTONS.iter().chain(GLOBAL_ERP).map(|b| b.g));
        hits.extend([DECK_SHIFT, DECK_SHIFT.on_deck(1), MIXER_SETTINGS_BTN, FX_ON]);
        for (i, &a) in hits.iter().enumerate() {
            for &b in &hits[i + 1..] {
                assert!(!overlap(a, b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn hits_follow_the_real_shape() {
        // The curved keys: the center is on each; the corner of the box nearest
        // the browse knob is not, because the key's lower edge follows the
        // knob's ring (low on the outer side, high in the middle).
        let a = crate::panel::ASPECT;
        for (k, inner) in [(TOP_BACK, 1.0), (TOP_TAG_TRACK, -1.0)] {
            assert!(k.contains(k.u, k.v));
            assert!(!k.contains(k.u + inner * k.hw * 0.9, k.v + k.hh * a * 0.9));
            assert!(k.contains(k.u - inner * k.hw * 0.7, k.v + k.hh * a * 0.75));
        }
        for (k, inner) in [(TOP_TRACK_FILTER, 1.0), (TOP_SHORTCUT, -1.0)] {
            assert!(k.contains(k.u, k.v));
            assert!(!k.contains(k.u + inner * k.hw * 0.9, k.v - k.hh * a * 0.9));
        }
        // A round button: the corner of its box is off it.
        let p = DECK_PLAY;
        assert!(p.contains(p.u, p.v));
        assert!(!p.contains(p.u + p.hw * 0.9, p.v + p.hh * a * 0.9));
    }

    #[test]
    fn every_led_field_is_its_own() {
        let mut seen = std::collections::HashSet::new();
        for &(g, byte, sh) in crate::controls::DECK_LEDS {
            assert!(
                byte < crate::controls::ERP_LED_DECK_STRIDE && sh % 2 == 0,
                "{g:?}"
            );
            assert!(seen.insert((byte, sh)), "{g:?}");
        }
        assert!(seen.insert(crate::controls::QUANTIZE_LED));
    }

    #[test]
    fn the_decks_are_copies() {
        // The right deck is the left one moved right, not a mirror image: its
        // IN/CUE is still left of OUT, and its 4 still left of its 8.
        assert!(DECK_IN_CUE.on_deck(1).u < DECK_OUT.on_deck(1).u);
        assert!(DECK_LOOP4.on_deck(1).u < DECK_LOOP8.on_deck(1).u);
        assert!(DECK_JOG.on_deck(1).u + DECK_JOG.hw < 1.0);
    }
}
