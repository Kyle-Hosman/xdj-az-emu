// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 The cdj3k-emu authors
//! Drawing parts ported from cdj3k-emu's front panel (see ATTRIBUTION.md): the
//! jog wheel, the tempo slider, the browse rotary and the jog feel knob, with the
//! ring and gear primitives they share.
//!
//! Upstream sizes every part in its own reference units. Here each part takes
//! `unit`, the screen size of one such unit, from the measured outline of the
//! matching XDJ-AZ control, so the part keeps upstream's proportions and the
//! XDJ-AZ's place and size.

use egui::epaint::{PathShape, TextShape};
use egui::{
    emath::Rot2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureId, Vec2,
};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

pub const COL_BLACK: Color32 = Color32::from_rgb(20, 20, 26);
pub const COL_DARK: Color32 = Color32::from_rgb(30, 30, 36);
pub const COL_BTN: Color32 = Color32::from_rgb(48, 48, 55);
pub const COL_SILVER: Color32 = Color32::from_rgb(100, 100, 110);
pub const COL_WHITE: Color32 = Color32::from_rgb(222, 222, 222);
pub const COL_TEXT: Color32 = Color32::from_rgb(200, 200, 210);
const COL_JOG_BODY: Color32 = Color32::from_rgb(35, 35, 40);
const COL_LCD_BG: Color32 = Color32::from_rgb(0, 0, 0);
/// The browse rotary's lit ring.
const COL_ENCODER_GLOW: Color32 = Color32::from_rgb(182, 220, 220);

// ---- Rings -----------------------------------------------------------------------------

/// Two concentric strokes: `inner` on the shape's edge, a clear `gap`, then
/// `outer` farther out.
#[derive(Clone, Copy)]
pub struct DoubleBorder {
    pub inner: Stroke,
    pub gap: f32,
    pub outer: Stroke,
}

impl DoubleBorder {
    /// How far the outer stroke's center sits outside the inner one's.
    fn spread(self) -> f32 {
        self.inner.width * 0.5 + self.gap.max(0.0) + self.outer.width * 0.5
    }
}

/// A circle of `radius` with a double border.
pub fn double_ring(p: &Painter, center: Pos2, radius: f32, b: DoubleBorder) {
    p.circle_stroke(center, radius + b.spread(), b.outer);
    p.circle_stroke(center, radius, b.inner);
}

/// A rounded rectangle with a double border.
pub fn double_rect(p: &Painter, rect: Rect, rounding: f32, b: DoubleBorder) {
    let e = b.spread();
    let outer = rect.expand(e);
    let max_r = 0.5 * outer.width().min(outer.height());
    p.rect_stroke(outer, (rounding + e).min(max_r), b.outer);
    p.rect_stroke(rect, rounding, b.inner);
}

// ---- Rotary controls: a gear knob, an indicator and a tick ring --------------------------

/// A gear knob outline: flat-topped teeth joined by circular-arc valleys. Each
/// valley lies on the circle through the two adjacent tooth-top corners (at the
/// outer radius) and the valley midpoint (at `base_radius`).
#[derive(Clone, Copy)]
pub struct Gear {
    /// The valley-bottom radius.
    pub base_radius: f32,
    pub tooth_count: usize,
    /// From `base_radius` to the flat tooth top.
    pub tooth_amplitude: f32,
    /// The share of each slot taken by the flat tooth top.
    pub tooth_top_fraction: f32,
    /// Straight chords per valley arc.
    pub valley_segments: usize,
    /// The rotation of the whole gear, radians, clockwise on screen.
    pub rotation: f32,
    pub fill: Color32,
    pub stroke: Stroke,
}

pub fn gear(p: &Painter, center: Pos2, g: Gear) {
    if g.tooth_count == 0 {
        return;
    }
    let r_min = g.base_radius;
    let r_max = g.base_radius + g.tooth_amplitude;
    let slot = TAU / g.tooth_count as f32;
    let tooth_half = slot * g.tooth_top_fraction.clamp(0.0, 1.0) * 0.5;
    let segments = g.valley_segments.max(1);

    // Every valley is congruent: compute its samples once in a valley-local frame
    // (+x along the valley's mid radial), then rotate them into place per tooth.
    let valley_half_span = 0.5 * (slot - 2.0 * tooth_half);
    let cos_half = valley_half_span.cos();
    let denom = r_max * cos_half - r_min;
    let use_arc = denom.abs() > 1.0e-4;
    let (x_c, arc_r, half_delta) = if use_arc {
        let x_c = (r_max * r_max - r_min * r_min) / (2.0 * denom);
        let arc_r = (r_min - x_c).abs();
        let half_delta = ((x_c - r_max * cos_half) / arc_r).clamp(-1.0, 1.0).acos();
        (x_c, arc_r, half_delta)
    } else {
        (0.0, 0.0, 0.0)
    };
    let valley: Vec<(f32, f32)> = (1..segments)
        .map(|k| {
            let t = k as f32 / segments as f32;
            if use_arc {
                let phi = PI - half_delta * (2.0 * t - 1.0);
                (x_c + arc_r * phi.cos(), arc_r * phi.sin())
            } else {
                // A degenerate arc is a straight V.
                let r = r_min + (r_max - r_min) * (2.0 * t - 1.0).abs();
                let a = (t - 0.5) * 2.0 * valley_half_span;
                (r * a.cos(), r * a.sin())
            }
        })
        .collect();

    let mut pts = Vec::with_capacity(g.tooth_count * (2 + valley.len()));
    for i in 0..g.tooth_count {
        let mid_tooth = (i as f32 + 0.5) * slot + g.rotation;
        for a in [mid_tooth - tooth_half, mid_tooth + tooth_half] {
            pts.push(center + Vec2::angled(a) * r_max);
        }
        let (s, c) = (mid_tooth + tooth_half + valley_half_span).sin_cos();
        for &(x, y) in &valley {
            pts.push(center + Vec2::new(x * c - y * s, x * s + y * c));
        }
    }
    // Star-shaped about the center, so the fill tessellates correctly.
    p.add(Shape::Path(PathShape {
        points: pts,
        closed: true,
        fill: g.fill,
        stroke: g.stroke.into(),
    }));
}

/// A unit vector `angle` radians clockwise from straight up.
fn from_top(angle: f32) -> Vec2 {
    Vec2::new(angle.sin(), -angle.cos())
}

/// An outlined pointer from the center outward, `angle` clockwise from up.
fn indicator(p: &Painter, center: Pos2, length: f32, width: f32, angle: f32, stroke: Stroke) {
    let dir = from_top(angle);
    let perp = Vec2::new(-dir.y, dir.x) * (width * 0.5);
    let pts = vec![
        center - perp,
        center + perp,
        center + perp + dir * length,
        center - perp + dir * length,
    ];
    p.add(Shape::closed_line(pts, stroke));
}

/// A tick ring along an arc: short radial lines at both ends, dots between them,
/// the middle dot a little larger.
#[derive(Clone, Copy)]
struct Ticks {
    count: usize,
    /// The first tick, radians clockwise from up, and the span to the last.
    start: f32,
    span: f32,
    radius: f32,
    dot: f32,
    center_dot: f32,
    endcap: f32,
    endcap_stroke: Stroke,
    color: Color32,
}

/// The endcaps sit a little outside the dots.
const ENDCAP_RADIAL_FACTOR: f32 = 1.2;

fn ticks(p: &Painter, center: Pos2, t: Ticks) {
    if t.count < 2 {
        return;
    }
    let last = t.count - 1;
    for i in 0..t.count {
        let dir = from_top(t.start + i as f32 / last as f32 * t.span);
        if i == 0 || i == last {
            let r = t.radius * ENDCAP_RADIAL_FACTOR;
            p.line_segment(
                [center + dir * r, center + dir * (r - t.endcap)],
                t.endcap_stroke,
            );
        } else {
            let r = if i == last / 2 { t.center_dot } else { t.dot };
            p.circle_filled(center + dir * t.radius, r, t.color);
        }
    }
}

/// The jog feel knob (upstream's JOG ADJUST): a 12-tooth gear, a pointer and a
/// 13-tick ring over 270 degrees. `unit` is one reference unit; the tick ring's
/// radius is [`ROTARY_TICK_RADIUS`] units. `pos` (0..1) turns the pointer and the
/// gear from the first tick to the last.
pub fn rotary_knob(p: &Painter, center: Pos2, unit: f32, pos: f32) {
    const START: f32 = 5.0 * PI / 4.0;
    const SPAN: f32 = 3.0 * PI / 2.0;
    let angle = START + pos.clamp(0.0, 1.0) * SPAN;
    let (base, amp) = (52.5 * unit, 7.0 * unit);
    gear(
        p,
        center,
        Gear {
            base_radius: base,
            tooth_count: 12,
            tooth_amplitude: amp,
            tooth_top_fraction: 0.35,
            valley_segments: 4,
            rotation: angle - FRAC_PI_2,
            fill: COL_BTN,
            stroke: Stroke::new(3.5 * unit, COL_TEXT),
        },
    );
    ticks(
        p,
        center,
        Ticks {
            count: 13,
            start: START,
            span: SPAN,
            radius: base + amp + 20.0 * unit,
            dot: 3.0 * unit,
            center_dot: 4.5 * unit,
            endcap: 25.0 * unit,
            endcap_stroke: Stroke::new(5.0 * unit, COL_TEXT),
            color: COL_TEXT,
        },
    );
    indicator(
        p,
        center,
        base * 1.2,
        7.0 * unit,
        angle,
        Stroke::new(5.0 * unit, COL_TEXT),
    );
}

/// The radius of [`rotary_knob`]'s tick ring, in its units.
pub const ROTARY_TICK_RADIUS: f32 = 79.5;

// ---- Browse rotary --------------------------------------------------------------------------

/// The radius, in [`browse_rotary`]'s units, of its outermost edge.
pub const BROWSE_RADIUS: f32 = 209.0;

/// The browse rotary: a 12-tooth gear turning with `angle` (radians), an inner
/// cap with a double ring, and around them a black bezel whose inner edge is the
/// encoder's lit ring. `pressed` darkens the gear and the cap.
pub fn browse_rotary(p: &Painter, center: Pos2, unit: f32, angle: f32, pressed: bool) {
    let (base, amp) = (100.0 * unit, 15.0 * unit);
    let bezel_in = base + amp + 12.0 * unit;
    let bezel_out = bezel_in + 82.0 * unit;
    // A stroke grows outward from its radius: this one fills the bezel.
    p.circle_stroke(
        center,
        bezel_in,
        Stroke::new(bezel_out - bezel_in, COL_BLACK),
    );
    let outline = Stroke::new(6.0 * unit, COL_SILVER);
    p.circle_stroke(center, bezel_out, outline);
    p.circle_stroke(center, bezel_in, outline);

    // The lit ring: halo layers, widest and faintest first, then the core.
    const GLOW_LAYERS: usize = 8;
    let (core, spread) = (8.0 * unit, 25.0 * unit);
    for layer in 0..GLOW_LAYERS {
        let t = 1.0 - layer as f32 / (GLOW_LAYERS - 1) as f32;
        let alpha = t * 0.28;
        if alpha >= 0.005 {
            let w = core + t * spread * 2.0;
            p.circle_stroke(
                center,
                bezel_in,
                Stroke::new(w, COL_ENCODER_GLOW.gamma_multiply(alpha)),
            );
        }
    }
    p.circle_stroke(center, bezel_in, Stroke::new(core, COL_ENCODER_GLOW));

    let fill = if pressed { COL_DARK } else { COL_BTN };
    gear(
        p,
        center,
        Gear {
            base_radius: base,
            tooth_count: 12,
            tooth_amplitude: amp,
            tooth_top_fraction: 0.35,
            valley_segments: 4,
            rotation: angle,
            fill,
            stroke: Stroke::new(7.0 * unit, COL_TEXT),
        },
    );
    let cap = 85.0 * unit;
    p.circle_filled(center, cap, fill);
    double_ring(
        p,
        center,
        cap,
        DoubleBorder {
            inner: Stroke::new(3.0 * unit, COL_SILVER),
            gap: 2.0 * unit,
            outer: Stroke::new(5.0 * unit, COL_SILVER),
        },
    );
}

// ---- Jog wheel ------------------------------------------------------------------------------

/// The radius, in [`jog_wheel`]'s units, of the jog's outer edge.
pub const JOG_RADIUS: f32 = 1060.0;

/// A jog wheel, center out: the jog display (see [`JogDisplay`]), the touch
/// platter to `touch`, the grip band of ovals and dots to `body`, and the outer
/// band to the edge, [`JOG_RADIUS`] units, where a white position mark turns with
/// `angle` (radians, clockwise, 0 = up). The grip turns with the platter.
pub struct Jog {
    pub center: Pos2,
    pub unit: f32,
    pub display: JogDisplay,
    pub touch: f32,
    pub body: f32,
    /// The grip's ovals: count, center radius, size along and across the ring.
    pub ovals: (usize, f32, f32, f32),
    /// The small circles after each oval: their size, and (degrees after the
    /// oval, radius) of each.
    pub dot: f32,
    pub dots: Vec<(f32, f32)>,
    pub angle: f32,
}

/// The jog display: a wide window with round ends inside the circle of radius
/// `outer`, and the artwork's circle, radius `art_r`, bulging above and below
/// it, showing `art`, or filled with `backdrop`. Around it, in the corners, the
/// SLIP, VINYL, SYNC and MASTER labels, lit or not as `lit` says (in that
/// order), at `label_size`.
pub struct JogDisplay {
    pub outer: f32,
    pub art_r: f32,
    pub art: Option<TextureId>,
    pub backdrop: Option<Color32>,
    pub lit: [bool; 4],
    pub label_size: f32,
}

/// The window's half-height as a share of the display's outer radius.
pub const DISPLAY_HALF_HEIGHT: f32 = 0.4;
/// The labels' ring as a share of the display's outer radius.
const DISPLAY_LABEL_RING: f32 = 0.87;
/// An unlit label.
const COL_LABEL_OFF: Color32 = Color32::from_rgb(70, 70, 78);

pub fn jog_wheel(p: &Painter, j: &Jog) {
    let (c, u) = (j.center, j.unit);
    let edge = JOG_RADIUS * u;
    p.circle_filled(c, edge, COL_SILVER);
    p.circle_filled(c, j.body, COL_JOG_BODY);
    double_ring(
        p,
        c,
        edge,
        DoubleBorder {
            inner: Stroke::new(5.0 * u, COL_WHITE),
            gap: 0.0,
            outer: Stroke::new(3.0 * u, COL_SILVER),
        },
    );

    // The grip: outlined ovals and dots, turning as one ring.
    let stroke = Stroke::new(6.0 * u, COL_TEXT);
    let (n, or, along, across) = j.ovals;
    for k in 0..n {
        let a = j.angle - FRAC_PI_2 + TAU * k as f32 / n as f32;
        let (radial, tangent) = (Vec2::angled(a), Vec2::angled(a + FRAC_PI_2));
        let at = c + radial * or;
        let pts: Vec<Pos2> = (0..32)
            .map(|q| {
                let (s, co) = (TAU * q as f32 / 32.0).sin_cos();
                at + radial * (across * 0.5 * co) + tangent * (along * 0.5 * s)
            })
            .collect();
        p.add(Shape::closed_line(pts, stroke));
        for &(deg, r) in &j.dots {
            let q = c + Vec2::angled(a + deg.to_radians()) * r;
            p.circle_stroke(q, j.dot * 0.5, stroke);
        }
    }

    p.circle_filled(c, j.touch, COL_DARK);
    double_ring(
        p,
        c,
        j.touch,
        DoubleBorder {
            inner: Stroke::new(5.0 * u, COL_TEXT),
            gap: 6.0 * u,
            outer: Stroke::new(3.0 * u, COL_SILVER),
        },
    );

    jog_display(p, c, u, &j.display);

    // The position mark: a slim quad across the outer band.
    let radial = from_top(j.angle);
    let tangent = Vec2::new(-radial.y, radial.x) * (8.0 * u);
    let pts = vec![
        c + radial * j.body + tangent,
        c + radial * edge + tangent,
        c + radial * edge - tangent,
        c + radial * j.body - tangent,
    ];
    p.add(Shape::convex_polygon(pts, COL_WHITE, Stroke::NONE));
}

/// The jog display's outline, center out at each angle: the window (a band
/// `2 * DISPLAY_HALF_HEIGHT` of the outer radius tall, ending on the outer
/// circle), or the artwork's circle where that reaches farther.
fn display_outline(c: Pos2, outer: f32, art_r: f32) -> Vec<Pos2> {
    const STEPS: usize = 720;
    let h = outer * DISPLAY_HALF_HEIGHT;
    (0..STEPS)
        .map(|k| {
            let d = Vec2::angled(TAU * k as f32 / STEPS as f32);
            let to_band = h / d.y.abs().max(1e-6);
            c + d * to_band.min(outer).max(art_r)
        })
        .collect()
}

fn jog_display(p: &Painter, c: Pos2, u: f32, d: &JogDisplay) {
    p.circle_filled(c, d.outer, COL_LCD_BG);
    let outline = display_outline(c, d.outer, d.art_r);
    if let Some(fill) = d.backdrop {
        p.circle_filled(c, d.art_r, fill);
    }
    if let Some(tex) = d.art {
        disc_image(p, c, d.art_r, tex);
    }
    p.add(Shape::closed_line(outline, Stroke::new(7.0 * u, COL_BTN)));

    // Clockwise from up, as on the deck: SLIP and VINYL over the window's
    // corners, MASTER and SYNC under them, those two turned to read upright.
    let ring = d.outer * DISPLAY_LABEL_RING;
    let font = FontId::proportional(d.label_size);
    let labels = [
        ("SLIP", 315.0, COL_WHITE, false),
        ("VINYL", 45.0, Color32::from_rgb(110, 200, 255), false),
        ("SYNC", 225.0, Color32::from_rgb(255, 150, 0), true),
        ("MASTER", 136.0, Color32::from_rgb(255, 120, 40), true),
    ];
    for ((text, deg, on, flip), lit) in labels.into_iter().zip(d.lit) {
        let col = if lit { on } else { COL_LABEL_OFF };
        curved_label(p, c, ring, deg, text, &font, col, 1.1 * u, flip);
    }
}

/// `text` along the circle of `radius` around `c`, centered `deg` degrees
/// clockwise from up, each glyph turned to the circle. `flip` turns the label to
/// read upright on the lower half. Each glyph gets a dark rim `rim` wide, so a
/// lit label reads on whatever is under it.
#[allow(clippy::too_many_arguments)]
fn curved_label(
    p: &Painter,
    c: Pos2,
    radius: f32,
    deg: f32,
    text: &str,
    font: &FontId,
    col: Color32,
    rim: f32,
    flip: bool,
) {
    let word = p.layout_no_wrap(text.to_owned(), font.clone(), col);
    let glyphs: Vec<(char, f32)> = word
        .rows
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| (g.chr, g.advance_width)))
        .collect();
    let total: f32 = glyphs.iter().map(|g| g.1).sum();
    if total < 1e-3 {
        return;
    }
    let span = (total / radius).min(0.9 * FRAC_PI_2);
    // One radial anchor for every glyph: the word's line center, so letters do
    // not shift with their own ink.
    let row_mid = word.rows.first().map_or(0.0, |r| r.rect.center().y);
    let mut along = 0.0;
    for (ch, adv) in glyphs {
        let mid = along + adv * 0.5;
        along += adv;
        if ch.is_whitespace() {
            continue;
        }
        let spread = (mid / total - 0.5) * span;
        let theta = deg.to_radians() + if flip { -spread } else { spread };
        let turn = theta + if flip { PI } else { 0.0 };
        let glyph = p.layout_no_wrap(ch.to_string(), font.clone(), col);
        let anchor = Vec2::new(glyph.mesh_bounds.center().x, row_mid);
        let pos = c + from_top(theta) * radius - Rot2::from_angle(turn) * anchor;
        for off in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
            let shadow = TextShape::new(pos + off * rim, glyph.clone(), COL_BTN)
                .with_override_text_color(COL_BTN)
                .with_angle(turn);
            p.add(Shape::Text(shadow));
        }
        let ts = TextShape::new(pos, glyph, col)
            .with_override_text_color(col)
            .with_angle(turn);
        p.add(Shape::Text(ts));
    }
}

/// A square image cropped to the disc of radius `r` around `c`.
fn disc_image(p: &Painter, c: Pos2, r: f32, tex: TextureId) {
    const STEPS: u32 = 64;
    let mut mesh = Mesh::with_texture(tex);
    mesh.colored_vertex(c, Color32::WHITE);
    mesh.vertices[0].uv = Pos2::new(0.5, 0.5);
    for k in 0..=STEPS {
        let (s, co) = (TAU * k as f32 / STEPS as f32).sin_cos();
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(c + Vec2::new(r * co, r * s), Color32::WHITE);
        mesh.vertices[i as usize].uv = Pos2::new(0.5 + 0.5 * co, 0.5 + 0.5 * s);
        if k > 0 {
            mesh.add_triangle(0, i - 1, i);
        }
    }
    p.add(Shape::mesh(mesh));
}

// ---- Tempo slider ---------------------------------------------------------------------------

/// The width, in [`tempo_slider`]'s units, of its backdrop.
pub const TEMPO_WIDTH: f32 = 238.8;

/// A tempo slider. `backdrop` is its surround; `top` and `bottom` the y of the
/// knob's center at the ends of its travel; `knob` the knob's size; `pos` 0 =
/// bottom, 1 = top. Beside it, left of the backdrop, a column of detents: `-` at
/// the top, `0` in the middle, `+` at the bottom, dots between.
pub struct Tempo {
    pub backdrop: Rect,
    pub top: f32,
    pub bottom: f32,
    pub knob: Vec2,
    pub unit: f32,
    pub pos: f32,
}

/// The detent column's place, in units left of the backdrop's left edge.
const TEMPO_TICK_OFFSET: f32 = 26.0;
/// Detents: the signs, the 0, and the dots, with one gap on each side of a sign.
const TEMPO_TICK_COUNT: usize = 51;

/// The tempo slider. The detent column's three signs (`-` at the top, `0`, `+`
/// at the bottom) are left to the caller, which prints them at its own text size.
pub fn tempo_slider(p: &Painter, t: &Tempo) {
    let u = t.unit;
    let b = t.backdrop;
    p.rect_filled(b, 36.0 * u, COL_BLACK);
    double_rect(
        p,
        b,
        36.0 * u,
        DoubleBorder {
            inner: Stroke::new(5.0 * u, COL_SILVER),
            gap: 0.0,
            outer: Stroke::new(3.0 * u, COL_DARK),
        },
    );
    // The two track bars.
    let cx = b.center().x;
    let inset = 110.0 * u;
    for x in [cx - 6.0 * u, cx + 6.0 * u] {
        p.line_segment(
            [
                Pos2::new(x, b.top() + inset),
                Pos2::new(x, b.bottom() - inset),
            ],
            Stroke::new(5.0 * u, COL_SILVER),
        );
    }
    // The scale ruled across the slider, a major line every fifth.
    let inner_w = b.width() - 10.0 * u;
    let tick_x = b.left() - TEMPO_TICK_OFFSET * u;
    for i in 0..TEMPO_TICK_COUNT {
        let y = t.top + (t.bottom - t.top) * i as f32 / (TEMPO_TICK_COUNT - 1) as f32;
        let major = i % 5 == 0;
        let half = inner_w * 0.5 * if major { 0.78 } else { 0.72 };
        let w = if major { 5.0 } else { 2.0 } * u;
        p.line_segment(
            [Pos2::new(cx - half, y), Pos2::new(cx + half, y)],
            Stroke::new(w, COL_TEXT),
        );
        let sign = matches!(i, 0 | 25) || i == TEMPO_TICK_COUNT - 1;
        let gap = matches!(i, 1 | 24 | 26 | 49);
        if !sign && !gap {
            p.circle_filled(Pos2::new(tick_x, y), 2.0 * u, COL_TEXT);
        }
    }
    // The knob: silver, a dark band across its middle, a white line on it.
    let y = t.bottom + (t.top - t.bottom) * t.pos.clamp(0.0, 1.0);
    let outer = Rect::from_center_size(Pos2::new(cx, y), t.knob);
    let inner = Rect::from_center_size(Pos2::new(cx, y), Vec2::new(t.knob.x, t.knob.y * 0.5));
    p.rect_filled(outer, 2.0 * u, COL_SILVER);
    p.rect_filled(inner, 1.0 * u, COL_BTN);
    let line = inner_w * 0.8 * 0.5;
    p.line_segment(
        [Pos2::new(cx - line, y), Pos2::new(cx + line, y)],
        Stroke::new(10.0 * u, COL_WHITE),
    );
}
