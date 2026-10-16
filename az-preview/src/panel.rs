// SPDX-License-Identifier: GPL-3.0-only
//! Drawing the front panel. Every control's place and size comes from
//! [`crate::layout`] (measured on the deck's top-panel drawing); [`Sc`] maps
//! the panel to the window.
//! Everything here only paints: the app (`app.rs`) does the hit-testing over
//! the same coordinates.

use crate::controls::{
    led_level, padmode_led, Mx, WaveColumn, DECK_LEDS, ERP_LED_DECK_STRIDE, MX_CH1_COLOR,
    MX_CH1_EQ_HI, MX_CH1_EQ_LOW, MX_CH1_EQ_MID, MX_CH1_FADER, MX_CH1_TRIM, MX_CH2_COLOR,
    MX_CH2_EQ_HI, MX_CH2_EQ_LOW, MX_CH2_EQ_MID, MX_CH2_FADER, MX_CH2_TRIM, MX_CH3_FADER,
    MX_CH4_FADER, MX_COUNT, MX_CROSSFADER, MX_MASTER_LEVEL, MX_SCFX_PARAM, QUANTIZE_LED,
};
use crate::layout::*;
use crate::meter::{Meter, CHANNEL_MARKS, MASTER_MARKS};
use crate::parts;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, TextureId, Vec2};
use std::f32::consts::TAU;

// ---- Palette ------------------------------------------------------------------------------
// The shared colors are cdj3k-emu's (see `parts`); these are the panel's own.
pub use crate::parts::{COL_BLACK, COL_SILVER};
use crate::parts::{COL_BTN, COL_DARK, COL_TEXT, COL_WHITE};
const COL_BLUE: Color32 = Color32::from_rgb(0, 102, 255);
const COL_CUE: Color32 = Color32::from_rgb(255, 150, 0);
/// The loop buttons' (IN/CUE, OUT, RELOOP/EXIT) yellow-orange.
const COL_LOOP: Color32 = Color32::from_rgb(255, 185, 0);
const COL_PLAY: Color32 = Color32::from_rgb(0, 200, 100);
const COL_RED: Color32 = Color32::from_rgb(230, 50, 50);
const COL_PAD: Color32 = Color32::from_rgb(14, 14, 18);
const COL_PANEL: Color32 = Color32::from_rgb(24, 24, 30);
/// Highlight for pressed and latched controls.
pub const COL_TEAL: Color32 = Color32::from_rgb(90, 230, 226);

/// The panel's aspect ratio (its reference size is 2742 x 1518, the size of
/// the drawing `layout/xdj-az.tsv` was measured on).
pub const ASPECT: f32 = 2742.0 / 1518.0;
/// Points of window left clear around the panel, so its outline is never cut
/// by the window's edge.
pub const PANEL_MARGIN: f32 = 6.0;
/// Scales line widths; controls are drawn at their true size.
const LINE_GAIN: f32 = 0.60;

/// Panel-to-window mapping: (u, v) in 0..1 inside `area`, letterboxed to
/// [`ASPECT`].
pub struct Sc {
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
}

impl Sc {
    pub fn new(area: Rect) -> Self {
        let r = fit_rect(area.shrink(PANEL_MARGIN), ASPECT);
        Sc {
            ox: r.left(),
            oy: r.top(),
            w: r.width(),
            h: r.height(),
        }
    }
    /// A point on the panel.
    pub fn p(&self, u: f32, v: f32) -> Pos2 {
        Pos2::new(self.ox + u * self.w, self.oy + v * self.h)
    }
    /// A line width or small offset given as a fraction of the panel width.
    pub fn l(&self, f: f32) -> f32 {
        f * self.w * LINE_GAIN
    }
    /// A length given as a fraction of the panel width, at true size.
    pub fn lr(&self, f: f32) -> f32 {
        f * self.w
    }
    /// The panel point (u, v) under a window position.
    pub fn uv(&self, q: Pos2) -> (f32, f32) {
        ((q.x - self.ox) / self.w, (q.y - self.oy) / self.h)
    }
    /// A control's center.
    pub fn c(&self, g: Geom) -> Pos2 {
        self.p(g.u, g.v)
    }
    /// A round control's radius.
    pub fn r(&self, g: Geom) -> f32 {
        self.lr(g.hw)
    }
    /// A control's outline.
    pub fn g(&self, g: Geom) -> Rect {
        Rect::from_center_size(
            self.c(g),
            Vec2::new(self.lr(g.hw) * 2.0, self.lr(g.hh) * 2.0),
        )
    }
}

/// The largest rect of aspect `aspect` (w/h) centered in `area`.
pub fn fit_rect(area: Rect, aspect: f32) -> Rect {
    let mut w = area.width();
    let mut h = w / aspect;
    if h > area.height() {
        h = area.height();
        w = h * aspect;
    }
    Rect::from_center_size(area.center(), Vec2::new(w, h))
}

/// Where the main screen sits on the panel: the visible glass, at the screen's
/// own 16:10, so a click maps to a screen pixel without stretching.
pub fn lcd_rect(sc: &Sc) -> Rect {
    sc.g(TOP_LCD)
}

/// The browse knob's center and radius.
pub fn browse_center(sc: &Sc) -> (Pos2, f32) {
    (sc.c(TOP_BROWSE_KNOB), sc.r(TOP_BROWSE_KNOB))
}

/// The X-PAD strip.
pub fn xpad_rect(sc: &Sc) -> Rect {
    sc.g(FX_XPAD)
}

/// The two working mixer channels: their controls, top to bottom (TRIM, HI,
/// MID, LOW, COLOR, fader), the panel place of each, and its mixer slot.
pub const CH_CONTROLS: [[(Geom, Mx); 6]; 2] = [
    [
        (MIXER_TRIM_1, MX_CH1_TRIM),
        (MIXER_HI_1, MX_CH1_EQ_HI),
        (MIXER_MID_1, MX_CH1_EQ_MID),
        (MIXER_LOW_1, MX_CH1_EQ_LOW),
        (MIXER_COLOR_1, MX_CH1_COLOR),
        (MIXER_FADER_1, MX_CH1_FADER),
    ],
    [
        (MIXER_TRIM_2, MX_CH2_TRIM),
        (MIXER_HI_2, MX_CH2_EQ_HI),
        (MIXER_MID_2, MX_CH2_EQ_MID),
        (MIXER_LOW_2, MX_CH2_EQ_LOW),
        (MIXER_COLOR_2, MX_CH2_COLOR),
        (MIXER_FADER_2, MX_CH2_FADER),
    ],
];

// ---- Type ------------------------------------------------------------------------------

/// The panel's three text sizes. All panel text (captions, section names) uses one
/// of them and never anything smaller; only a label on a button too small for it
/// is shrunk below [`Type::Caption`] to fit (see [`fit_label`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Type {
    /// Captions printed beside a control, sub-labels and meter scales.
    Caption,
    /// Labels printed on buttons, and section names.
    Button,
    /// Channel numbers, the large transport buttons and the model name.
    Heading,
}

impl Type {
    /// Every size, smallest first.
    const ALL: [Type; 3] = [Type::Caption, Type::Button, Type::Heading];

    /// The size as a fraction of the panel width: about 7, 10 and 14 points on a
    /// panel 1280 points wide.
    fn frac(self) -> f32 {
        match self {
            Type::Caption => 0.0055,
            Type::Button => 0.0078,
            Type::Heading => 0.0108,
        }
    }

    fn font(self, sc: &Sc) -> FontId {
        FontId::proportional(sc.lr(self.frac()))
    }
}

// ---- Primitives -------------------------------------------------------------------------

/// Panel text: always one of the type scale's sizes.
fn text(p: &Painter, sc: &Sc, pos: Pos2, s: &str, ty: Type, col: Color32, anchor: Align2) {
    let font = ty.font(sc);
    debug_assert!(font.size >= Type::Caption.font(sc).size);
    p.text(pos, anchor, s, font, col);
}

/// The size the printed text's places are given in: reference pixels of the
/// drawing `layout/xdj-az.tsv` was measured on (see [`ASPECT`]).
const REF_W: f32 = 2742.0;
const REF_H: f32 = 1518.0;

/// Text printed on the panel, centered on `at`: its place on the drawing, in
/// reference pixels. Each line is centered on its own, the block on `at`.
fn print(p: &Painter, sc: &Sc, at: (f32, f32), s: &str, ty: Type) {
    print_in(p, sc, at, s, ty, COL_TEXT);
}

/// [`print`] in a color of its own.
fn print_in(p: &Painter, sc: &Sc, (x, y): (f32, f32), s: &str, ty: Type, col: Color32) {
    let row = p.fonts(|f| f.row_height(&ty.font(sc)));
    let lines: Vec<&str> = s.split('\n').collect();
    let c = sc.p(x / REF_W, y / REF_H);
    let y0 = c.y - row * (lines.len() as f32 - 1.0) * 0.5;
    for (i, line) in lines.iter().enumerate() {
        let at = Pos2::new(c.x, y0 + i as f32 * row);
        text(p, sc, at, line, ty, col, Align2::CENTER_CENTER);
    }
}

/// How far deck `d`'s printed text sits from the left deck's, in reference
/// pixels: the decks are copies, shifted as their controls are.
fn deck_dx(d: usize) -> f32 {
    if d == 0 {
        0.0
    } else {
        DECK_R_DX * REF_W
    }
}

// ---- Outlines -------------------------------------------------------------------------

/// A control's outline on screen: its traced path, its circle or its rounded
/// rectangle, as measured (see [`crate::layout::Look`]).
/// `inset` moves a circle's or rectangle's outline inward (a traced path is kept
/// as traced).
fn outline(sc: &Sc, g: Geom, inset: f32) -> Vec<Pos2> {
    let c = sc.c(g);
    if !g.look.path.is_empty() {
        return g
            .look
            .path
            .iter()
            .map(|&(x, y)| c + Vec2::new(sc.lr(x), sc.lr(y)))
            .collect();
    }
    if g.round {
        return circle_pts(c, sc.r(g) - inset, 64);
    }
    let (hw, hh) = (sc.lr(g.hw) - inset, sc.lr(g.hh) - inset);
    let r = (sc.lr(g.look.corner) - inset).max(0.0).min(hw).min(hh);
    let mut pts = Vec::with_capacity(28);
    for (cx, cy, a0) in [
        (hw - r, hh - r, 0.0),
        (-(hw - r), hh - r, 0.25),
        (-(hw - r), -(hh - r), 0.5),
        (hw - r, -(hh - r), 0.75),
    ] {
        for k in 0..=6 {
            let a = TAU * (a0 + 0.25 * k as f32 / 6.0);
            pts.push(c + Vec2::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    pts
}

// epaint strokes circles and rectangles outside their edge. These place a
// stroke on the measured geometry instead: an outline's outer edge on the
// control's measured edge, a printed line centered on its measured radius.

/// A circle's outline, its outer edge at radius `r`.
fn edge_circle(p: &Painter, c: Pos2, r: f32, stroke: Stroke) {
    p.circle_stroke(c, (r - stroke.width).max(0.0), stroke);
}

/// A printed circle (a ring), centered on radius `r`.
fn line_circle(p: &Painter, c: Pos2, r: f32, stroke: Stroke) {
    p.circle_stroke(c, (r - stroke.width * 0.5).max(0.0), stroke);
}

/// A rectangle's outline, its outer edge on `rect`.
fn edge_rect(p: &Painter, rect: Rect, rounding: f32, stroke: Stroke) {
    let w = stroke.width;
    p.rect_stroke(rect.shrink(w), (rounding - w).max(0.0), stroke);
}

fn circle_pts(c: Pos2, r: f32, n: usize) -> Vec<Pos2> {
    (0..n)
        .map(|k| {
            let a = TAU * k as f32 / n as f32;
            c + Vec2::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

/// Fill and stroke a closed outline, convex or not.
fn shape(p: &Painter, pts: Vec<Pos2>, fill: Color32, stroke: Stroke) {
    let mut mesh = egui::Mesh::default();
    for &q in &pts {
        mesh.colored_vertex(q, fill);
    }
    mesh.indices = triangulate(&pts);
    p.add(Shape::mesh(mesh));
    p.add(Shape::closed_line(pts, stroke));
}

/// Triangle indices for a simple polygon, by ear clipping (the traced keys are
/// concave, and a convex fill would spill over their curved edge).
fn triangulate(pts: &[Pos2]) -> Vec<u32> {
    let area: f32 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let ccw = area > 0.0;
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::with_capacity(3 * pts.len());
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    while idx.len() > 3 {
        let n = idx.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (
                pts[idx[(i + n - 1) % n]],
                pts[idx[i]],
                pts[idx[(i + 1) % n]],
            );
            let turn = cross(a, b, c);
            if (turn > 0.0) != ccw || turn == 0.0 {
                return false;
            }
            idx.iter().all(|&j| {
                let q = pts[j];
                if q == a || q == b || q == c {
                    return true;
                }
                let (d1, d2, d3) = (cross(a, b, q), cross(b, c, q), cross(c, a, q));
                let inside = if ccw {
                    d1 > 0.0 && d2 > 0.0 && d3 > 0.0
                } else {
                    d1 < 0.0 && d2 < 0.0 && d3 < 0.0
                };
                !inside
            })
        });
        // A degenerate outline has no clean ear: fan the rest rather than loop.
        let i = ear.unwrap_or(1);
        out.extend([idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]].map(|v| v as u32));
        idx.remove(i);
    }
    out.extend(idx.iter().map(|&v| v as u32));
    out
}

/// Button style.
#[derive(Clone, Copy)]
enum Bs {
    Dark,
    Blue,
    Outlined,
}

/// A button in its measured shape. `lit` is the color of its LED when the
/// deck program has it on (None: off, or a button without one). Returns the
/// color its label is printed in.
fn rbtn(p: &Painter, sc: &Sc, g: Geom, style: Bs, lit: Option<Color32>) -> Color32 {
    let (fill, border, txt) = match style {
        Bs::Dark => (COL_BTN, COL_SILVER, COL_TEXT),
        Bs::Blue => (COL_BLUE, COL_BLUE, COL_WHITE),
        Bs::Outlined => (COL_DARK, COL_WHITE, COL_WHITE),
    };
    let fill = lit.map_or(fill, |c| c.gamma_multiply(0.55));
    // A closed line is stroked on its path: inset by half its width, the
    // outline's outer edge is the button's.
    let stroke = Stroke::new(sc.l(0.0012).max(1.0), border);
    shape(p, outline(sc, g, stroke.width * 0.5), fill, stroke);
    lit.map_or(txt, |_| COL_WHITE)
}

/// [`rbtn`] with `label` printed on it (see [`fit_label`]).
fn rbtn_text(
    p: &Painter,
    sc: &Sc,
    g: Geom,
    label: &str,
    ty: Type,
    style: Bs,
    lit: Option<Color32>,
) {
    let txt = rbtn(p, sc, g, style, lit);
    let rect = sc.g(g);
    let (size, lines) = fit_label(p, sc, rect, label, ty);
    label_in(p, rect, &lines, size, txt);
}

/// Buttons of one group with their labels at one size: the largest that fits
/// on every one of them, so a group never mixes sizes.
fn rbtn_row(
    p: &Painter,
    sc: &Sc,
    row: &[(Geom, &str)],
    ty: Type,
    style: Bs,
    lit: &[Option<Color32>],
) {
    let fits: Vec<(f32, String)> = row
        .iter()
        .map(|&(g, s)| fit_label(p, sc, sc.g(g), s, ty))
        .collect();
    let size = fits.iter().map(|f| f.0).fold(f32::INFINITY, f32::min);
    for (i, (&(g, _), (_, lines))) in row.iter().zip(&fits).enumerate() {
        let txt = rbtn(p, sc, g, style, lit.get(i).copied().flatten());
        label_in(p, sc.g(g), lines, size, txt);
    }
}

/// How a button label is printed: its lines, and the size. Words may go on one
/// line or one per line, whichever prints larger. The size is `ty` when that
/// fits in `rect`, else the largest smaller size of the type scale that does,
/// else (a button too small for any of them) whatever does fit.
fn fit_label(p: &Painter, sc: &Sc, rect: Rect, s: &str, ty: Type) -> (f32, String) {
    let layouts = [s.to_owned(), s.replace('\n', " "), s.replace(' ', "\n")];
    let best = layouts
        .iter()
        .map(|l| (label_room(p, rect, l), l))
        .fold((0.0, &layouts[0]), |a, b| if b.0 > a.0 { b } else { a });
    let (room, lines) = (best.0, best.1.clone());
    let size = Type::ALL
        .iter()
        .rev()
        .skip_while(|&&t| t != ty)
        .map(|t| t.font(sc).size)
        .find(|&size| size <= room)
        .unwrap_or(room);
    (size, lines)
}

/// The largest size at which `s` (lines split at `\n`) fits in `rect`, with a
/// little room inside the outline.
fn label_room(p: &Painter, rect: Rect, s: &str) -> f32 {
    // Text scales linearly: measure once at a large size.
    const PROBE: f32 = 100.0;
    let font = FontId::proportional(PROBE);
    let w = s
        .split('\n')
        .map(|l| {
            p.layout_no_wrap(l.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    let h = p.fonts(|f| f.row_height(&font)) * s.split('\n').count() as f32;
    (rect.width() * LABEL_ROOM / w).min(rect.height() * 0.95 / h) * PROBE
}

/// How much of a button's width its label may use.
const LABEL_ROOM: f32 = 0.88;

/// `s` centered in `rect` at `size` points, one line per `\n`.
fn label_in(p: &Painter, rect: Rect, s: &str, size: f32, col: Color32) {
    let font = FontId::proportional(size);
    let lines: Vec<&str> = s.split('\n').collect();
    let row = p.fonts(|f| f.row_height(&font));
    let y0 = rect.center().y - row * (lines.len() as f32 - 1.0) * 0.5;
    for (i, line) in lines.iter().enumerate() {
        let pos = Pos2::new(rect.center().x, y0 + i as f32 * row);
        p.text(pos, Align2::CENTER_CENTER, line, font.clone(), col);
    }
}

/// Round button colors: (ring, label).
#[derive(Clone, Copy)]
enum Ring {
    Plain,
    Cue,
    Play,
    Blue,
    Loop,
}

/// A round button: its measured rings and filled center, and the LED ring lit
/// when the deck program has it on. Returns the color its label is printed in.
fn cbtn(p: &Painter, sc: &Sc, g: Geom, ring: Ring, lit: bool) -> Color32 {
    let (ring_col, txt) = match ring {
        Ring::Plain => (COL_SILVER, COL_TEXT),
        Ring::Cue => (COL_CUE, COL_CUE),
        Ring::Play => (COL_PLAY, COL_PLAY),
        Ring::Blue => (COL_BLUE, COL_WHITE),
        Ring::Loop => (COL_LOOP, COL_TEXT),
    };
    let (c, r) = (sc.c(g), sc.r(g));
    p.circle_filled(c, r, COL_BTN);
    edge_circle(p, c, r, Stroke::new(sc.l(0.0016).max(1.0), COL_SILVER));
    // The rings inside the edge, center out. Lit, the band they span is one
    // continuous light ring; unlit, they are the button's printed outlines.
    let inner = &g.look.rings[..g.look.rings.len().saturating_sub(1)];
    match (lit, inner.first(), inner.last()) {
        (true, Some(&r0), Some(&r1)) => {
            let width = sc.lr(r1 - r0).max(sc.l(0.0016)).max(1.5);
            line_circle(p, c, sc.lr(r0 + r1) * 0.5, Stroke::new(width, ring_col));
        }
        _ => {
            for &rr in inner {
                line_circle(
                    p,
                    c,
                    sc.lr(rr),
                    Stroke::new(sc.l(0.0010).max(0.75), COL_DARK),
                );
            }
        }
    }
    if g.look.fill > 0.0 {
        p.circle_filled(c, sc.lr(g.look.fill), COL_BLACK);
    }
    txt
}

/// [`cbtn`] with `label` printed in it (see [`fit_label`]).
fn cbtn_text(p: &Painter, sc: &Sc, g: Geom, label: &str, ty: Type, ring: Ring, lit: bool) {
    let txt = cbtn(p, sc, g, ring, lit);
    // About the square inside the edge.
    let rect = Rect::from_center_size(sc.c(g), Vec2::splat(sc.r(g) * 1.5));
    let (size, lines) = fit_label(p, sc, rect, label, ty);
    label_in(p, rect, &lines, size, txt);
}

#[derive(Clone, Copy)]
enum Dir {
    L,
    R,
}

/// A filled triangle (the font has no arrow glyphs).
fn tri(p: &Painter, center: Pos2, half: f32, dir: Dir, col: Color32) {
    let s = match dir {
        Dir::L => -1.0,
        Dir::R => 1.0,
    };
    let pts = vec![
        Pos2::new(center.x + s * half, center.y),
        Pos2::new(center.x - s * half, center.y - half),
        Pos2::new(center.x - s * half, center.y + half),
    ];
    p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// A round button with one or two arrows.
fn arrow_cbtn(p: &Painter, sc: &Sc, g: Geom, dir: Dir, double: bool) {
    cbtn(p, sc, g, Ring::Plain, false);
    let c = sc.c(g);
    let h = sc.r(g) * 0.30;
    if double {
        tri(p, Pos2::new(c.x - h, c.y), h, dir, COL_TEXT);
        tri(p, Pos2::new(c.x + h, c.y), h, dir, COL_TEXT);
    } else {
        tri(p, c, h, dir, COL_TEXT);
    }
}

/// A knob as measured: its printed scale dots and arc, its outer rings, a
/// knurled band or scalloped skirt, the cap and the pointer. `pos` (0..1) turns
/// the pointer through about 300 degrees; None leaves it pointing up.
pub fn knob(p: &Painter, sc: &Sc, g: Geom, pos: Option<f32>) {
    let (c, r) = (sc.c(g), sc.r(g));
    let look = g.look;
    let at = |deg: f32, rr: f32| {
        let a = deg.to_radians();
        c + Vec2::new(rr * a.cos(), rr * a.sin())
    };
    let thin = Stroke::new(sc.l(0.0010).max(0.75), COL_SILVER);
    // Printed around the knob: the scale dots and the range arc.
    for &deg in look.dots.1 {
        p.circle_filled(
            at(deg, sc.lr(look.dots.0)),
            sc.lr(0.0007).max(1.0),
            COL_SILVER,
        );
    }
    for &(a0, a1) in look.arc.2 {
        let n = ((a1 - a0) / 3.0).ceil().max(2.0) as usize;
        let pts: Vec<Pos2> = (0..=n)
            .map(|k| at(a0 + (a1 - a0) * k as f32 / n as f32, sc.lr(look.arc.0)))
            .collect();
        p.add(Shape::line(pts, Stroke::new(sc.lr(look.arc.1), COL_SILVER)));
    }
    // The skirt: scalloped, or a smooth outer ring.
    let (lobes, s_in, s_out) = look.scallop;
    if lobes > 0 {
        let n = lobes as usize * 8;
        let pts: Vec<Pos2> = (0..n)
            .map(|k| {
                let t = k as f32 / n as f32;
                let wave = 0.5 + 0.5 * (TAU * lobes as f32 * t).cos();
                let rr = sc.lr(s_in) + (sc.lr(s_out) - sc.lr(s_in)) * (0.55 + 0.45 * wave);
                at(360.0 * t, rr)
            })
            .collect();
        shape(p, pts, COL_DARK, thin);
    } else {
        p.circle_filled(c, r, COL_DARK);
        edge_circle(p, c, r, thin);
    }
    for &rr in look.rings.iter().rev().skip(if lobes > 0 { 0 } else { 1 }) {
        line_circle(p, c, sc.lr(rr), thin);
    }
    let (grips, gr) = look.grips;
    for k in 0..grips {
        let q = at(360.0 * k as f32 / grips as f32, sc.lr(gr));
        line_circle(p, q, sc.lr(gr) * TAU / grips as f32 * 0.32, thin);
    }
    // The cap: the innermost ring, or most of the knob without one.
    let cap = look.rings.first().map_or(r * 0.62, |&rr| sc.lr(rr));
    p.circle_filled(c, cap, COL_BTN);
    let ang = pos.map_or(-90.0, |t| -90.0 + (t.clamp(0.0, 1.0) - 0.5) * 300.0);
    p.line_segment(
        [at(ang, cap * 0.15), at(ang, cap * 0.95)],
        Stroke::new(sc.l(0.0016).max(1.0), COL_WHITE),
    );
}

/// The browse knob (see [`parts::browse_rotary`]): turning with `angle`
/// (radians), pressed while `flash` (0..1) is up.
fn browse_knob(p: &Painter, sc: &Sc, angle: f32, flash: f32) {
    let (c, r) = browse_center(sc);
    parts::browse_rotary(p, c, r / parts::BROWSE_RADIUS, angle, flash > 0.0);
}

/// A vertical fader: `track` is its travel (the cap's center moves between
/// its ends), `cap` the cap's size; `pos` 0 = bottom, 1 = top.
fn vfader(p: &Painter, sc: &Sc, track: Geom, cap: Geom, pos: f32) {
    let (v_top, v_bot) = track.travel_v();
    let (top, bot) = (sc.p(track.u, v_top), sc.p(track.u, v_bot));
    p.line_segment([top, bot], Stroke::new(sc.l(0.002).max(1.0), COL_SILVER));
    let at = Pos2::new(top.x, bot.y + (top.y - bot.y) * pos);
    fader_cap(p, sc, Rect::from_center_size(at, sc.g(cap).size()));
}

/// The crossfader; `pos` 0 = A (left), 1 = B.
fn hfader(p: &Painter, sc: &Sc, pos: f32) {
    let (u_l, u_r) = MIXER_XFADER.travel_u();
    let (l, r) = (sc.p(u_l, MIXER_XFADER.v), sc.p(u_r, MIXER_XFADER.v));
    p.line_segment([l, r], Stroke::new(sc.l(0.002).max(1.0), COL_SILVER));
    let at = Pos2::new(l.x + (r.x - l.x) * pos, l.y);
    fader_cap(
        p,
        sc,
        Rect::from_center_size(at, sc.g(MIXER_XFADER_CAP).size()),
    );
}

fn fader_cap(p: &Painter, sc: &Sc, cap: Rect) {
    p.rect_filled(cap, sc.l(0.002), COL_WHITE);
    edge_rect(p, cap, sc.l(0.002), Stroke::new(1.0_f32, COL_SILVER));
}

/// The jog display's background behind the overview waveform.
const COL_JOG_WAVE_BG: Color32 = Color32::from_rgb(72, 72, 78);

/// What the jog display shows in the artwork's circle.
#[derive(Clone, Copy)]
enum JogView {
    /// The track's artwork, or nothing until it arrives.
    Art(Option<TextureId>),
    /// The overview waveform's gray background (the waveform is drawn over it).
    Waveform,
}

/// A jog wheel (see [`parts::jog_wheel`]) on its measured rings: the touch
/// platter's edge, the grip band's outer edge, the outer edge. The grip ring and
/// the position mark turn with `angle` (radians, clockwise); the jog display
/// (showing `view`) and its SLIP, VINYL, SYNC and MASTER labels (lit as `lit`
/// says) stay upright.
fn jog(p: &Painter, sc: &Sc, g: Geom, angle: f32, view: JogView, lit: [bool; 4]) {
    let Some(j) = &g.look.jog else { return };
    let [.., touch, body, edge] = g.look.rings else {
        return;
    };
    let (n, or, along, across) = j.ovals;
    parts::jog_wheel(
        p,
        &parts::Jog {
            center: sc.c(g),
            unit: sc.lr(*edge) / parts::JOG_RADIUS,
            display: parts::JogDisplay {
                outer: sc.lr(j.hub.1),
                art_r: sc.lr(j.hub.0),
                art: match view {
                    JogView::Art(tex) => tex,
                    JogView::Waveform => None,
                },
                backdrop: matches!(view, JogView::Waveform).then_some(COL_JOG_WAVE_BG),
                lit,
                label_size: Type::Caption.font(sc).size,
            },
            touch: sc.lr(*touch),
            body: sc.lr(*body),
            ovals: (n as usize, sc.lr(or), sc.lr(along), sc.lr(across)),
            dot: sc.lr(j.small),
            dots: j.smalls.iter().map(|&(deg, r)| (deg, sc.lr(r))).collect(),
            angle,
        },
    );
}

/// The overview waveform along the bottom of the jog display's window, as the
/// deck draws it there, with the playing position. Each column is its color in
/// its lower, brighter part and a dimmer one above.
fn jog_overview(p: &Painter, sc: &Sc, g: Geom, cols: &[WaveColumn], position: Option<f32>) {
    let Some(j) = &g.look.jog else { return };
    if cols.is_empty() {
        return;
    }
    let (c, outer) = (sc.c(g), sc.lr(j.hub.1));
    let h = outer * parts::DISPLAY_HALF_HEIGHT;
    // Where the deck puts it, as shares of the window's half-height below the
    // center: the strip's top and bottom, and the line under it.
    let (top, bottom, base) = (c.y + 0.49 * h, c.y + 0.89 * h, c.y + 0.93 * h);
    let half_w = 0.96 * (outer * outer - (base - c.y).powi(2)).max(0.0).sqrt();
    let (left, width) = (c.x - half_w, 2.0 * half_w);
    // The tallest column seen was 39.
    let unit = (bottom - top) / 40.0;
    let col_w = width / cols.len() as f32;
    for (i, col) in cols.iter().enumerate() {
        let x = left + i as f32 * col_w;
        let bar = |hgt: u8, color: Color32| {
            let r = Rect::from_min_max(
                Pos2::new(x, bottom - f32::from(hgt) * unit),
                Pos2::new(x + col_w, bottom),
            );
            p.rect_filled(r, 0.0, color);
        };
        bar(col.height, col.color.gamma_multiply(0.5));
        bar(col.inner.min(col.height), col.color);
    }
    let line = Stroke::new(sc.l(0.0016).max(1.0), COL_WHITE);
    p.line_segment([Pos2::new(left, base), Pos2::new(left + width, base)], line);
    if let Some(t) = position {
        let x = left + width * t;
        p.line_segment([Pos2::new(x, top), Pos2::new(x, base)], line);
    }
}

/// The tempo slider (see [`parts::tempo_slider`]) in its measured frame, with
/// its detent signs at the caption size.
fn tempo_slider(p: &Painter, sc: &Sc, d: usize, pos: f32) {
    let frame = sc.g(DECK_TEMPO_FRAME.on_deck(d));
    let track = DECK_TEMPO_TRACK.on_deck(d);
    let (v_top, v_bot) = track.travel_v();
    let t = parts::Tempo {
        backdrop: frame,
        top: sc.p(track.u, v_top).y,
        bottom: sc.p(track.u, v_bot).y,
        knob: sc.g(DECK_TEMPO_CAP).size(),
        unit: frame.width() / parts::TEMPO_WIDTH,
        pos,
    };
    parts::tempo_slider(p, &t);
    // The signs beside the detent column, as printed.
    for (y, sign) in [(1130.0, "\u{2212}"), (1279.5, "0"), (1429.0, "+")] {
        print(p, sc, (827.5 + deck_dx(d), y), sign, Type::Caption);
    }
}

/// A level meter: `g.look.segs` segments along its long side, the first
/// `lit` of them (from the bottom, or the left) lit. The colors are not on
/// the drawing: green, amber from the 0 dB mark, red for the top segment.
fn level_meter(p: &Painter, sc: &Sc, g: Geom, marks: &[f32], lit: usize) {
    let r = sc.g(g);
    let (n, len) = (g.look.segs.0 as usize, sc.lr(g.look.segs.1));
    let vertical = r.height() > r.width();
    let total = if vertical { r.height() } else { r.width() };
    let pitch = if n > 1 {
        (total - len) / (n - 1) as f32
    } else {
        0.0
    };
    let stroke = Stroke::new(sc.l(0.0006).max(0.75), COL_SILVER);
    for k in 0..n {
        let along = len * 0.5 + pitch * k as f32;
        let (center, size) = if vertical {
            (
                Pos2::new(r.center().x, r.bottom() - along),
                Vec2::new(r.width(), len),
            )
        } else {
            (
                Pos2::new(r.left() + along, r.center().y),
                Vec2::new(len, r.height()),
            )
        };
        let seg = Rect::from_center_size(center, size);
        let col = if k >= lit {
            COL_DARK
        } else if k + 1 == n {
            COL_RED
        } else if marks.get(k).is_some_and(|&m| m >= 0.0) {
            COL_CUE
        } else {
            COL_PLAY
        };
        p.rect_filled(seg, 0.0, col);
        edge_rect(p, seg, 0.0, stroke);
    }
}

// ---- Panel sections -----------------------------------------------------------------------

/// Mixer channels 1-4's level meters, with their printed marks.
fn channel_meters(p: &Painter, sc: &Sc, meters: &[Meter; 4]) {
    let strips = [MIXER_METER_1, MIXER_METER_2, MIXER_METER_3, MIXER_METER_4];
    for (g, m) in strips.into_iter().zip(meters) {
        level_meter(p, sc, g, &CHANNEL_MARKS, m.lit(&CHANNEL_MARKS));
        // The scale is printed right of each strip, level with its segments.
        let r = sc.g(g);
        let pitch = (r.height() - sc.lr(g.look.segs.1)) / (CHANNEL_MARKS.len() - 1) as f32;
        // Every other mark: at the caption size, all of them do not fit.
        for (k, mark) in CHANNEL_MARKS.iter().enumerate().step_by(2) {
            let y = r.bottom() - sc.lr(g.look.segs.1) * 0.5 - pitch * k as f32;
            let pos = Pos2::new(r.right() + sc.lr(3.0 / REF_W), y);
            text(
                p,
                sc,
                pos,
                &format!("{mark}"),
                Type::Caption,
                COL_TEXT,
                Align2::LEFT_CENTER,
            );
        }
    }
}

/// What one deck shows live.
#[derive(Clone, Copy, Default)]
pub struct DeckView<'a> {
    /// Tempo fader, 0 = bottom, 1 = top.
    pub tempo: f32,
    /// Jog platter rotation, radians.
    pub jog_angle: f32,
    /// Pad LED colors from the deck program (black = off).
    pub pad_led: [Color32; 8],
    /// The jog display's image, as the deck program last sent it.
    pub art: Option<TextureId>,
    /// The jog display's overview waveform, and the playing position (0..1).
    pub overview: &'a [WaveColumn],
    pub position: Option<f32>,
    /// The jog display shows the artwork even when a waveform has arrived (a
    /// double press of SHIFT switches it, as on the deck).
    pub show_art: bool,
}

/// One deck (`d` = 0 left, 1 right). The decks are copies of each other: the
/// right one is the left one shifted right ([`Geom::on_deck`]).
fn deck(p: &Painter, sc: &Sc, d: usize, view: &DeckView, erp: &[u8]) {
    let at = |g: Geom| g.on_deck(d);
    // This deck's LED level for a button (0 when it has no LED we know).
    let led = |g: Geom| {
        DECK_LEDS
            .iter()
            .find(|(b, _, _)| *b == g)
            .map_or(0, |&(_, byte, sh)| {
                led_level(erp, byte + d * ERP_LED_DECK_STRIDE, sh)
            })
    };
    let lit = |g: Geom, col: Color32| match led(g) {
        0 => None,
        1 => Some(col.gamma_multiply(0.45)),
        _ => Some(col),
    };

    // First: everything else sits on the jog's outline.
    // The jog display's SLIP, VINYL, SYNC and MASTER labels follow the buttons'
    // LEDs. Level 1 is a button's dim "off" (VINYL off, SYNC after use, the
    // other deck's MASTER); SLIP blinks between 0 and 2 while on.
    let display_lit = [
        led(DECK_SLIP) > 0,
        led(DECK_JOG_MODE) > 1,
        led(DECK_BEAT_SYNC) > 1,
        led(DECK_MASTER) > 1,
    ];
    // No message says which view the deck's jog display is in, so it follows the
    // data: the overview waveform once it has arrived, else the artwork, unless
    // a double press of SHIFT asked for the artwork.
    let waveform = !view.overview.is_empty() && !view.show_art;
    let jog_view = if waveform {
        JogView::Waveform
    } else {
        JogView::Art(view.art)
    };
    jog(p, sc, at(DECK_JOG), view.jog_angle, jog_view, display_lit);
    if waveform {
        jog_overview(p, sc, at(DECK_JOG), view.overview, view.position);
    }

    // This deck's printed text, at its place on the drawing.
    let dx = deck_dx(d);
    let cap = |x: f32, y: f32, s: &str| print(p, sc, (x + dx, y), s, Type::Caption);
    // The top row's captions share one line, as printed.
    cbtn(p, sc, at(DECK_IN_CUE), Ring::Loop, led(DECK_IN_CUE) > 0);
    cap(95.5, 445.0, "IN/CUE");
    cbtn(p, sc, at(DECK_OUT), Ring::Loop, led(DECK_OUT) > 0);
    cap(172.0, 445.0, "OUT");
    cbtn(p, sc, at(DECK_RELOOP), Ring::Loop, led(DECK_RELOOP) > 0);
    cap(264.0, 445.0, "RELOOP/EXIT");
    // The deck numbers sit in each key's inner panel, not at its center.
    let sel = if d == 0 { ("3", "1") } else { ("2", "4") };
    let txt = rbtn(p, sc, at(DECK_SELECT_L), Bs::Dark, None);
    print_in(p, sc, (442.0 + dx, 487.0), sel.0, Type::Button, txt);
    let txt = rbtn(p, sc, at(DECK_SELECT_R), Bs::Outlined, None);
    print_in(p, sc, (539.0 + dx, 487.0), sel.1, Type::Button, txt);
    cap(490.0, 445.0, "DECK SELECT");
    arrow_cbtn(p, sc, at(DECK_CALL_PREV), Dir::L, false);
    arrow_cbtn(p, sc, at(DECK_CALL_NEXT), Dir::R, false);
    cap(712.0, 451.0, "CUE/LOOP\nCALL");
    cbtn(p, sc, at(DECK_DELETE), Ring::Plain, false);
    // DELETE and MEMORY are printed closer than the caption size allows: each
    // is moved a few pixels apart.
    cap(810.0, 445.0, "DELETE");
    cbtn(p, sc, at(DECK_MEMORY), Ring::Plain, false);
    cap(875.0, 445.0, "MEMORY");

    cbtn_text(p, sc, at(DECK_LOOP4), "4", Type::Button, Ring::Plain, false);
    cbtn_text(p, sc, at(DECK_LOOP8), "8", Type::Button, Ring::Plain, false);
    // Printed between the two buttons; at the caption size it does not fit
    // there, so it sits over them.
    cap(134.0, 531.0, "BEAT LOOP");
    rbtn(p, sc, at(DECK_JUMP_PREV), Bs::Dark, None);
    tri(
        p,
        sc.c(at(DECK_JUMP_PREV)),
        sc.lr(DECK_JUMP_PREV.hh) * 0.45,
        Dir::L,
        COL_TEXT,
    );
    rbtn(p, sc, at(DECK_JUMP_NEXT), Bs::Dark, None);
    tri(
        p,
        sc.c(at(DECK_JUMP_NEXT)),
        sc.lr(DECK_JUMP_NEXT.hh) * 0.45,
        Dir::R,
        COL_TEXT,
    );
    // As BEAT LOOP: over the keys, not between them.
    cap(134.0, 618.0, "BEAT JUMP");
    // SLIP, VINYL SPEED ADJUST and MASTER TEMPO stay black on the deck: the
    // program sends their LED bits, but nothing on the panel lights.
    cbtn(p, sc, at(DECK_SLIP), Ring::Plain, false);
    cap(96.0, 740.5, "SLIP");
    rbtn_text(
        p,
        sc,
        at(DECK_SLIP_REV),
        "SLIP REV",
        Type::Caption,
        Bs::Dark,
        None,
    );

    // JOG FEEL (not driven: the emulator has no jog tension), centered.
    let feel = at(DECK_JOG_FEEL);
    let ring = sc.lr(feel.look.dots.0);
    parts::rotary_knob(p, sc.c(feel), ring / parts::ROTARY_TICK_RADIUS, 0.5);
    cap(762.0, 570.5, "JOG FEEL");
    cbtn(p, sc, at(DECK_VINYL_SPEED), Ring::Plain, false);
    cap(872.5, 533.0, "VINYL\nSPEED ADJUST");
    rbtn_text(
        p,
        sc,
        at(DECK_JOG_MODE),
        "VINYL",
        Type::Button,
        Bs::Dark,
        lit(DECK_JOG_MODE, COL_BLUE),
    );
    cap(872.5, 615.0, "JOG MODE");
    rbtn_text(
        p,
        sc,
        at(DECK_BEAT_SYNC),
        "BEAT\nSYNC",
        Type::Caption,
        Bs::Dark,
        lit(DECK_BEAT_SYNC, COL_CUE),
    );
    rbtn_text(
        p,
        sc,
        at(DECK_MASTER),
        "MASTER",
        Type::Caption,
        Bs::Dark,
        lit(DECK_MASTER, COL_CUE),
    );
    rbtn_text(
        p,
        sc,
        at(DECK_KEY_SYNC),
        "KEY SYNC",
        Type::Caption,
        Bs::Dark,
        lit(DECK_KEY_SYNC, COL_CUE),
    );
    cbtn(p, sc, at(DECK_TEMPO_RANGE), Ring::Plain, false);
    cap(873.0, 936.5, "TEMPO");
    cbtn(p, sc, at(DECK_MASTER_TEMPO), Ring::Plain, false);
    cap(872.5, 1012.0, "MASTER\nTEMPO");

    rbtn_text(
        p,
        sc,
        at(DECK_SHIFT),
        "SHIFT",
        Type::Button,
        Bs::Outlined,
        None,
    );
    arrow_cbtn(p, sc, at(DECK_TRACK_PREV), Dir::L, true);
    arrow_cbtn(p, sc, at(DECK_TRACK_NEXT), Dir::R, true);
    cap(135.0, 1071.0, "TRACK SEARCH");
    arrow_cbtn(p, sc, at(DECK_SEARCH_REV), Dir::L, true);
    arrow_cbtn(p, sc, at(DECK_SEARCH_FWD), Dir::R, true);
    cap(135.0, 1142.5, "SEARCH");
    cbtn_text(
        p,
        sc,
        at(DECK_CUE),
        "CUE",
        Type::Heading,
        Ring::Cue,
        led(DECK_CUE) > 0,
    );
    cbtn(p, sc, at(DECK_PLAY), Ring::Play, led(DECK_PLAY) > 0);
    tri(
        p,
        sc.c(at(DECK_PLAY)),
        sc.r(DECK_PLAY) * 0.25,
        Dir::R,
        COL_PLAY,
    );

    let modes = [
        (DECK_PADMODE1, "HOT CUE", "GATE CUE"),
        (DECK_PADMODE2, "BEAT LOOP", "BEAT LOOP 2"),
        (DECK_PADMODE3, "SLIP LOOP", "KEY SHIFT"),
        (DECK_PADMODE4, "BEAT JUMP", "BEAT JUMP 2"),
    ];
    let row = modes.map(|(g, m, _)| (at(g), m));
    // Each mode button lit in the color the deck program sends for it.
    let mode_lit: Vec<Option<Color32>> = (0..4).map(|j| padmode_led(erp, d, j)).collect();
    rbtn_row(p, sc, &row, Type::Caption, Bs::Dark, &mode_lit);
    for &(g, _, sub) in &modes {
        cap(REF_W * g.u, 1265.5, sub);
    }
    cap(491.5, 1218.0, "PAD MODE");
    let pads = [
        DECK_PAD1, DECK_PAD2, DECK_PAD3, DECK_PAD4, DECK_PAD5, DECK_PAD6, DECK_PAD7, DECK_PAD8,
    ];
    for (&g, &led) in pads.iter().zip(view.pad_led.iter()) {
        let rect = sc.g(at(g));
        p.rect_filled(rect, sc.l(0.004), COL_PAD);
        // The border is the deck program's own LED color; silver when off.
        let border = if led == Color32::BLACK {
            COL_SILVER
        } else {
            led
        };
        edge_rect(
            p,
            rect,
            sc.l(0.004),
            Stroke::new(sc.l(0.0025).max(1.5), border),
        );
    }

    tempo_slider(p, sc, d, view.tempo);
    cbtn(p, sc, at(DECK_TEMPO_RESET), Ring::Plain, false);
    cap(771.0, 1241.0, "TEMPO\nRESET");
}

/// One mixer column: its number, its controls top to bottom (TRIM, HI, MID,
/// LOW, COLOR, CUE, fader), and the live slots when it has 8-bit controls.
type MixerColumn = (
    &'static str,
    [Geom; 7],
    Option<&'static [(Geom, Mx); 6]>,
    Mx,
);

/// The mixer, columns 3, 1, 2, 4 from the left. Every fader and channel 1 and
/// 2's knobs show their live values; channel 3 and 4's knobs are drawn live by
/// the app over these.
fn mixer(p: &Painter, sc: &Sc, pos: &[f32; MX_COUNT]) {
    let cols: [MixerColumn; 4] = [
        (
            "3",
            [
                MIXER_TRIM_3,
                MIXER_HI_3,
                MIXER_MID_3,
                MIXER_LOW_3,
                MIXER_COLOR_3,
                MIXER_CUE_3,
                MIXER_FADER_3,
            ],
            None,
            MX_CH3_FADER,
        ),
        (
            "1",
            [
                MIXER_TRIM_1,
                MIXER_HI_1,
                MIXER_MID_1,
                MIXER_LOW_1,
                MIXER_COLOR_1,
                MIXER_CUE_1,
                MIXER_FADER_1,
            ],
            Some(&CH_CONTROLS[0]),
            MX_CH1_FADER,
        ),
        (
            "2",
            [
                MIXER_TRIM_2,
                MIXER_HI_2,
                MIXER_MID_2,
                MIXER_LOW_2,
                MIXER_COLOR_2,
                MIXER_CUE_2,
                MIXER_FADER_2,
            ],
            Some(&CH_CONTROLS[1]),
            MX_CH2_FADER,
        ),
        (
            "4",
            [
                MIXER_TRIM_4,
                MIXER_HI_4,
                MIXER_MID_4,
                MIXER_LOW_4,
                MIXER_COLOR_4,
                MIXER_CUE_4,
                MIXER_FADER_4,
            ],
            None,
            MX_CH4_FADER,
        ),
    ];
    // Each knob's caption, and the channel number, at their printed heights.
    let captions = [
        ("TRIM", 608.5),
        ("HI", 692.0),
        ("MID", 783.0),
        ("LOW", 874.0),
        ("COLOR", 983.0),
    ];
    for (name, g, live, fader) in cols {
        let x = REF_W * g[0].u;
        for (row, &(caption, y)) in captions.iter().enumerate() {
            knob(p, sc, g[row], live.map(|c| pos[c[row].1]));
            print(p, sc, (x, y), caption, Type::Caption);
        }
        // The EQ curve's mode, printed beside HI over the level meter.
        print(p, sc, (x - 36.0, 686.0), "EQ/\nISO", Type::Caption);
        rbtn_text(p, sc, g[5], "CUE", Type::Button, Bs::Dark, None);
        print(p, sc, (x, 1149.0), name, Type::Heading);
        vfader(p, sc, g[6], MIXER_FADER_CAP, pos[fader]);
    }
    let scfx = [
        (MIXER_SCFX_SPACE, "SPACE"),
        (MIXER_SCFX_DUBECHO, "DUB\nECHO"),
        (MIXER_SCFX_CRUSH, "CRUSH"),
        (MIXER_SCFX_SWEEP, "SWEEP"),
        (MIXER_SCFX_NOISE, "NOISE"),
        (MIXER_SCFX_FILTER, "FILTER"),
    ];
    rbtn_row(p, sc, &scfx, Type::Caption, Bs::Dark, &[]);
    // Printed as large as the other section names, but at that size it is
    // wider than its section: the caption size fits.
    print(p, sc, (1004.0, 755.5), "SOUND COLOR FX", Type::Caption);
    knob(p, sc, MIXER_SCFX_PARAM, Some(pos[MX_SCFX_PARAM]));
    print(p, sc, (1004.5, 983.0), "PARAMETER", Type::Caption);
    // The headphone MIX and LEVEL knobs (mixer bus 0x4c, 0x4d) are drawn but not
    // driven: the emulator has no headphone output.
    rbtn_text(
        p,
        sc,
        MIXER_LINK_CUE,
        "LINK\nCUE",
        Type::Caption,
        Bs::Dark,
        None,
    );
    print(p, sc, (1004.0, 1096.0), "HEADPHONES", Type::Button);
    knob(p, sc, MIXER_HP_MIX, None);
    print(p, sc, (1004.0, 1168.5), "MIX", Type::Caption);
    knob(p, sc, MIXER_HP_LEVEL, None);
    print(p, sc, (1004.5, 1274.0), "LEVEL", Type::Caption);
    rbtn_text(
        p,
        sc,
        MIXER_SETTINGS_BTN,
        "\u{2699}",
        Type::Button,
        Bs::Dark,
        None,
    );
    print(p, sc, (1004.0, 617.0), "MIXER\nSETTINGS", Type::Button);
    hfader(p, sc, pos[MX_CROSSFADER]);
    print(p, sc, (1214.0, 1442.5), "A", Type::Button);
    print(p, sc, (1448.5, 1442.5), "B", Type::Button);
}

fn beatfx(p: &Painter, sc: &Sc) {
    let strip = xpad_rect(sc);
    p.rect_filled(strip, sc.l(0.002), COL_BLACK);
    edge_rect(p, strip, sc.l(0.002), Stroke::new(1.0_f32, COL_SILVER));
    print(p, sc, (1702.0, 609.5), "BEAT FX", Type::Button);
    print(p, sc, (1702.0, 627.5), "X-PAD", Type::Caption);
    rbtn(p, sc, FX_BEAT_PREV, Bs::Dark, None);
    tri(
        p,
        sc.c(FX_BEAT_PREV),
        sc.lr(FX_BEAT_PREV.hh) * 0.5,
        Dir::L,
        COL_TEXT,
    );
    rbtn(p, sc, FX_BEAT_NEXT, Bs::Dark, None);
    tri(
        p,
        sc.c(FX_BEAT_NEXT),
        sc.lr(FX_BEAT_NEXT.hh) * 0.5,
        Dir::R,
        COL_TEXT,
    );
    print(p, sc, (1702.0, 738.0), "BEAT", Type::Caption);
    cbtn(p, sc, FX_AUTO_TAP, Ring::Plain, false);
    print(p, sc, (1644.5, 772.0), "AUTO/TAP", Type::Caption);
    cbtn_text(p, sc, FX_TAP, "TAP", Type::Caption, Ring::Plain, false);
    cbtn(p, sc, FX_QUANTIZE, Ring::Plain, false);
    // At the caption size, FX QUANTIZE would touch TAP: it moves right.
    print(p, sc, (1768.0, 772.0), "FX QUANTIZE", Type::Caption);
    rbtn_text(p, sc, FX_FREQ_LOW, "LOW", Type::Caption, Bs::Dark, None);
    rbtn_text(p, sc, FX_FREQ_MID, "MID", Type::Caption, Bs::Dark, None);
    rbtn_text(p, sc, FX_FREQ_HI, "HI", Type::Caption, Bs::Blue, None);
    print(p, sc, (1702.0, 839.0), "FX FREQUENCY", Type::Caption);
    // The FX SELECT knob itself is drawn live by the app.
    fx_names(p, sc);
    let chans = [
        (FX_CH_3, "3"),
        (FX_CH_1, "1"),
        (FX_CH_2, "2"),
        (FX_CH_4, "4"),
    ];
    let sources = [
        (FX_CH_MIC, "MIC"),
        (FX_CH_CFA, "CF.A"),
        (FX_CH_CFB, "CF.B"),
        (FX_CH_MST, "MST"),
    ];
    let row: Vec<(Geom, &str)> = chans.into_iter().chain(sources).collect();
    rbtn_row(p, sc, &row, Type::Caption, Bs::Dark, &[]);
    print(p, sc, (1702.0, 1103.5), "CH SELECT", Type::Caption);
    knob(p, sc, FX_TIME, None);
    print(p, sc, (1702.0, 1168.0), "TIME", Type::Caption);
    knob(p, sc, FX_LEVEL_DEPTH, None);
    print(p, sc, (1702.5, 1274.5), "LEVEL/DEPTH", Type::Caption);
    cbtn(p, sc, FX_ON, Ring::Blue, false);
    print(p, sc, (1702.5, 1382.5), "ON/OFF", Type::Caption);
}

/// The effect names around the FX SELECT knob. The deck prints them smaller than
/// any caption, so they are the one text below the type scale: as large as
/// FLANGER's measured width allows, and each no wider than its own.
fn fx_names(p: &Painter, sc: &Sc) {
    // (name, left and right edge, center height) on the drawing; the left
    // column is aligned on its right edges, the right column on its left ones.
    const NAMES: [(&str, f32, f32, f32); 14] = [
        ("FLANGER", 1657.0, 1698.0, 922.5),
        ("REVERB", 1645.0, 1680.0, 935.5),
        ("HELIX", 1642.0, 1666.0, 948.5),
        ("SPIRAL", 1632.0, 1662.0, 963.0),
        ("PING PONG", 1614.0, 1664.0, 978.0),
        ("ECHO", 1644.0, 1669.0, 991.5),
        ("DELAY", 1651.0, 1678.0, 1005.5),
        ("PHASER", 1708.0, 1743.0, 922.5),
        ("FILTER", 1728.0, 1756.0, 935.5),
        ("TRIPLET FILTER", 1738.0, 1791.0, 948.5),
        ("TRANS", 1742.0, 1771.0, 963.0),
        ("ROLL", 1743.0, 1765.0, 978.0),
        ("TRIPLET ROLL", 1738.0, 1784.0, 991.5),
        ("MOBIUS", 1727.0, 1761.0, 1005.5),
    ];
    // Text scales linearly: measure once at a large size.
    const PROBE: f32 = 100.0;
    let size_for = |s: &str, ref_w: f32| {
        let w = p
            .layout_no_wrap(s.to_owned(), FontId::proportional(PROBE), COL_TEXT)
            .size()
            .x;
        PROBE * (ref_w / REF_W * sc.w) / w
    };
    let common = size_for(NAMES[0].0, NAMES[0].2 - NAMES[0].1);
    for (i, &(name, x0, x1, y)) in NAMES.iter().enumerate() {
        let size = common.min(size_for(name, x1 - x0));
        let (x, anchor) = if i < 7 {
            (x1, Align2::RIGHT_CENTER)
        } else {
            (x0, Align2::LEFT_CENTER)
        };
        p.text(
            sc.p(x / REF_W, y / REF_H),
            anchor,
            name,
            FontId::proportional(size),
            COL_TEXT,
        );
    }
}

/// The screen-tab row, the screen and the browse cluster.
fn top_center(p: &Painter, sc: &Sc, browse_angle: f32, browse_flash: f32) {
    let tabs = [
        (TOP_SOURCE, "SOURCE"),
        (TOP_BROWSE, "BROWSE"),
        (TOP_TAG_LIST, "TAG LIST"),
        (TOP_PLAYLIST, "PLAYLIST"),
        (TOP_SEARCH, "SEARCH"),
        (TOP_MENU, "MENU"),
    ];
    rbtn_row(p, sc, &tabs, Type::Caption, Bs::Dark, &[]);
    // The bezel, without an outline: the screen sits in a plain dark frame.
    p.rect_filled(sc.g(TOP_BEZEL), sc.l(0.006), COL_DARK);
    let lcd = lcd_rect(sc);
    p.rect_filled(lcd, 0.0, COL_BLACK);
    let msg = "MAIN LCD  1280\u{00d7}800";
    p.text(
        lcd.center(),
        Align2::CENTER_CENTER,
        msg,
        Type::Heading.font(sc),
        COL_SILVER,
    );

    // The four keys around the browse knob. They must match
    // controls::PANEL_BUTTONS (both read the same layout entries).
    // Each label sits where it is printed, in the key's straight part (the
    // keys curve around the knob); `h` is the room it has there.
    for (g, l, at, h) in [
        (TOP_BACK, "BACK", (1728.5, 174.5), 20.0),
        (TOP_TAG_TRACK, "TAG TRACK\n/REMOVE", (1778.5, 174.5), 24.0),
        (TOP_TRACK_FILTER, "FILTER\n/EDIT", (1730.0, 366.0), 28.0),
        (TOP_SHORTCUT, "SHORT\nCUT", (1778.0, 362.5), 28.0),
    ] {
        let txt = rbtn(p, sc, g, Bs::Dark, None);
        let c = sc.p(at.0 / REF_W, at.1 / REF_H);
        let rect = Rect::from_center_size(c, Vec2::new(sc.g(g).width(), sc.lr(h / REF_W)));
        let (size, lines) = fit_label(p, sc, rect, l, Type::Caption);
        label_in(p, rect, &lines, size, txt);
    }
    browse_knob(p, sc, browse_angle, browse_flash);
    for (g, l) in [
        (TOP_LOAD1, "1"),
        (TOP_LOAD2, "2"),
        (TOP_LOAD3, "3"),
        (TOP_LOAD4, "4"),
    ] {
        rbtn_text(p, sc, g, l, Type::Button, Bs::Dark, None);
    }
    print(p, sc, (1754.0, 428.5), "LOAD", Type::Caption);
}

/// MIC (top left), MASTER and USB (top right).
fn top_sides(p: &Painter, sc: &Sc, erp: &[u8], master_level: f32) {
    // Each row: its ON button, its number right of it, its knobs and their
    // captions (the y they are printed at).
    let mic = [
        (
            MIC_ON1,
            [MIC_LEVEL1, MIC_LOW1, MIC_MID1, MIC_HI1],
            "1",
            269.0,
            234.0,
        ),
        (
            MIC_ON2,
            [MIC_LEVEL2, MIC_LOW2, MIC_MID2, MIC_HI2],
            "2",
            357.0,
            319.5,
        ),
    ];
    for (on, knobs, n, n_y, cap_y) in mic {
        cbtn_text(p, sc, on, "ON", Type::Caption, Ring::Plain, false);
        print(p, sc, (303.5, n_y), n, Type::Heading);
        for (g, cap) in knobs.into_iter().zip(["LEVEL", "LOW", "MID", "HI"]) {
            knob(p, sc, g, None);
            print(p, sc, (REF_W * g.u, cap_y), cap, Type::Caption);
        }
    }
    print(p, sc, (510.0, 220.5), "MIC", Type::Button);
    cbtn(p, sc, MIC_TALK_OVER, Ring::Plain, false);
    print(p, sc, (311.5, 314.5), "TALK\nOVER", Type::Caption);
    // QUANTIZE shows it is on with a red dot in its center, as on the deck.
    cbtn(p, sc, GLOBAL_QUANTIZE, Ring::Plain, false);
    if led_level(erp, QUANTIZE_LED.0, QUANTIZE_LED.1) > 0 {
        let (c, r) = (sc.c(GLOBAL_QUANTIZE), sc.r(GLOBAL_QUANTIZE));
        p.circle_filled(c, r * 0.36, COL_RED);
    }
    print(p, sc, (872.5, 291.5), "QUANTIZE", Type::Caption);
    print(p, sc, (872.0, 340.5), "WAKE UP", Type::Caption);

    knob(p, sc, MASTER_LEVEL, Some(master_level));
    print(p, sc, (2004.5, 236.0), "LEVEL", Type::Caption);
    rbtn_text(p, sc, MASTER_CUE, "CUE", Type::Button, Bs::Dark, None);
    for (g, cap) in [(MASTER_LOW, "LOW"), (MASTER_MID, "MID"), (MASTER_HI, "HI")] {
        knob(p, sc, g, None);
        print(p, sc, (REF_W * g.u, 319.0), cap, Type::Caption);
    }
    knob(p, sc, MASTER_BOOTH, None);
    print(p, sc, (2378.0, 313.0), "BOOTH\nLEVEL", Type::Caption);
    print(p, sc, (2148.5, 220.5), "MASTER", Type::Button);
    // The deck program never computes the master mix: the mixer runs after
    // its output, in the DSP. With nothing real to show, the master meter
    // stays dark.
    let frame = Stroke::new(sc.l(0.0010).max(0.75), COL_SILVER);
    edge_rect(p, sc.g(MASTER_METER), sc.lr(4.0 / REF_W), frame);
    for (row, name, y) in [(MASTER_METER_L, "L", 253.5), (MASTER_METER_R, "R", 290.0)] {
        level_meter(p, sc, row, &MASTER_MARKS, 0);
        print(p, sc, (2061.5, y), name, Type::Caption);
    }
    // Every other mark of the scale between the rows, where it is printed: at the
    // caption size, all of them do not fit side by side.
    for (x, m) in [
        (2083.0, "-24"),
        (2125.0, "-15"),
        (2166.5, "-9"),
        (2209.0, "-3"),
        (2247.5, "3"),
        (2289.0, "9"),
    ] {
        print(p, sc, (x, 272.0), m, Type::Caption);
    }
    for (slot, stop, name, x) in [
        (USB_SLOT1, USB_STOP1, "USB 1", 2488.5),
        (USB_SLOT2, USB_STOP2, "USB 2", 2619.5),
    ] {
        let r = sc.g(slot);
        p.rect_filled(r, sc.l(0.003), COL_DARK);
        edge_rect(p, r, sc.l(0.003), Stroke::new(1.0_f32, COL_SILVER));
        print(p, sc, (x, 224.0), name, Type::Caption);
        cbtn(p, sc, stop, Ring::Plain, false);
        print(p, sc, (x + 5.0, 333.0), "USB STOP", Type::Caption);
    }
}

/// Everything live on the panel, for one paint.
pub struct PanelView<'a> {
    pub decks: [DeckView<'a>; 2],
    pub mixer: &'a [f32; MX_COUNT],
    pub browse_angle: f32,
    pub browse_flash: f32,
    /// The deck program's latest erp LED frame.
    pub erp: &'a [u8],
    /// Mixer channels 1-4's level meters.
    pub meters: &'a [Meter; 4],
}

/// Paint the whole panel into `area`.
pub fn paint(p: &Painter, area: Rect, view: &PanelView) {
    let sc = Sc::new(area);
    p.rect_filled(area, 0.0, COL_BLACK);
    // The body, and the screen's housing rising above it.
    for hull in [sc.g(PANEL_BODY), sc.g(PANEL_TOWER)] {
        p.rect_filled(hull, sc.l(0.01), COL_PANEL);
        edge_rect(
            p,
            hull,
            sc.l(0.01),
            Stroke::new(sc.l(0.004).max(1.0), COL_SILVER),
        );
    }
    // The housing's inside, over the body's top edge.
    let w = sc.l(0.004).max(1.0);
    p.rect_filled(sc.g(PANEL_TOWER).shrink(w), sc.l(0.01) - w, COL_PANEL);
    top_center(p, &sc, view.browse_angle, view.browse_flash);
    top_sides(p, &sc, view.erp, view.mixer[MX_MASTER_LEVEL]);
    deck(p, &sc, 0, &view.decks[0], view.erp);
    deck(p, &sc, 1, &view.decks[1], view.erp);
    mixer(p, &sc, view.mixer);
    channel_meters(p, &sc, view.meters);
    beatfx(p, &sc);
    print_in(p, &sc, (1004.0, 1471.0), "XDJ-AZ", Type::Heading, COL_WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_is_the_smallest_panel_text() {
        // Panel text is drawn only at these sizes, so nothing outside a button
        // is ever smaller than a caption; ALL runs smallest first.
        for pair in Type::ALL.windows(2) {
            assert!(pair[0].frac() < pair[1].frac(), "{pair:?}");
        }
        assert_eq!(Type::ALL[0], Type::Caption);
    }
}
