// SPDX-License-Identifier: GPL-3.0-only
//! Turns layout/xdj-az.tsv (control positions and shapes in panel pixels) into
//! constants: one `Geom` per control, with its `Look`, as fractions of the
//! panel, so the drawing and the hit-testing read the same numbers and the
//! tables stay `const`.

use std::fmt::Write as _;
use std::{env, fs, path::Path};

const SRC: &str = "layout/xdj-az.tsv";
/// The panel's reference size in the source drawing (matches `panel::ASPECT`).
const PANEL_W: f64 = 2742.0;
const PANEL_H: f64 = 1518.0;

/// A length in panel pixels, as a fraction of the panel width.
fn fw(px: f64) -> String {
    format!("{:.6}", px / PANEL_W)
}

fn num(s: &str, at: &str) -> f64 {
    s.trim()
        .parse()
        .unwrap_or_else(|_| panic!("{at}: bad number {s:?}"))
}

/// `a-b` as two numbers.
fn span(s: &str, at: &str) -> (f64, f64) {
    let (a, b) = s
        .split_once('-')
        .unwrap_or_else(|| panic!("{at}: expected a-b, got {s:?}"));
    (num(a, at), num(b, at))
}

/// `n@rest` as a count and the rest.
fn count<'a>(s: &'a str, at: &str) -> (u32, &'a str) {
    let (n, rest) = s
        .split_once('@')
        .unwrap_or_else(|| panic!("{at}: expected n@..., got {s:?}"));
    (num(n, at) as u32, rest)
}

/// The `Look` literal for one row's optional seventh field.
fn look(spec: &str, at: &str) -> String {
    let mut rings = String::new();
    let (mut fill, mut corner) = (String::from("0.0"), String::from("0.0"));
    let mut grips = String::from("(0, 0.0)");
    let mut segs = String::from("(0, 0.0)");
    let mut scallop = String::from("(0, 0.0, 0.0)");
    let mut dots = String::from("(0.0, &[])");
    let mut arc = String::from("(0.0, 0.0, &[])");
    let mut path = String::new();
    let mut jog = String::from("None");
    let (mut hub, mut ovals, mut smalls) = (None, None, None);
    for item in spec.split(';').filter(|s| !s.is_empty()) {
        let (key, val) = item
            .split_once('=')
            .unwrap_or_else(|| panic!("{at}: expected key=value, got {item:?}"));
        match key {
            "rings" => rings = val.split(',').map(|r| fw(num(r, at)) + ", ").collect(),
            "fill" => fill = fw(span(val, at).1),
            "corner" => corner = fw(num(val, at)),
            "grips" => {
                let (n, r) = count(val, at);
                grips = format!("({n}, {})", fw(num(r, at)));
            }
            "segs" => {
                let (n, len) = val
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{at}: expected n:len, got {val:?}"));
                segs = format!("({}, {})", num(n, at) as u32, fw(num(len, at)));
            }
            "scallop" => {
                let (n, r) = count(val, at);
                let (a, b) = span(r, at);
                scallop = format!("({n}, {}, {})", fw(a), fw(b));
            }
            "dots" => {
                let (r, angles) = val
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{at}: dots=r:a,b,..."));
                let list: String = angles
                    .split(',')
                    .map(|a| format!("{:.1}, ", num(a, at)))
                    .collect();
                dots = format!("({}, &[{list}])", fw(num(r, at)));
            }
            "arc" => {
                let (rw, spans) = val
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{at}: arc=r,w:a-b,..."));
                let (r, w) = rw
                    .split_once(',')
                    .unwrap_or_else(|| panic!("{at}: arc=r,w:..."));
                let list: String = spans
                    .split(',')
                    .map(|s| {
                        let (a, b) = span(s, at);
                        format!("({a:.1}, {b:.1}), ")
                    })
                    .collect();
                arc = format!("({}, {}, &[{list}])", fw(num(r, at)), fw(num(w, at)));
            }
            "path" => {
                path = val
                    .split_whitespace()
                    .map(|p| {
                        let (x, y) = p
                            .split_once(',')
                            .unwrap_or_else(|| panic!("{at}: path point x,y"));
                        format!("({}, {}), ", fw(num(x, at)), fw(num(y, at)))
                    })
                    .collect()
            }
            "hub" => hub = Some(span(val, at)),
            "ovals" => {
                let (n, rest) = count(val, at);
                let (r, wh) = rest
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{at}: ovals=n@r:wxh"));
                let (w, h) = wh
                    .split_once('x')
                    .unwrap_or_else(|| panic!("{at}: ovals size wxh"));
                ovals = Some((n, num(r, at), num(w, at), num(h, at)));
            }
            "smalls" => {
                let (size, list) = val
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{at}: smalls=size:deg@r,..."));
                let pts: Vec<(f64, f64)> = list
                    .split(',')
                    .map(|p| {
                        let (d, r) = p
                            .split_once('@')
                            .unwrap_or_else(|| panic!("{at}: smalls deg@r"));
                        (num(d, at), num(r, at))
                    })
                    .collect();
                smalls = Some((num(size, at), pts));
            }
            other => panic!("{at}: unknown look key {other:?}"),
        }
    }
    if let (Some(h), Some((n, r, w, hh)), Some((size, pts))) = (hub, ovals, smalls) {
        let list: String = pts
            .iter()
            .map(|(d, r)| format!("({d:.1}, {}), ", fw(*r)))
            .collect();
        jog = format!(
            "Some(JogLook {{ hub: ({}, {}), ovals: ({n}, {}, {}, {}), small: {}, smalls: &[{list}] }})",
            fw(h.0), fw(h.1), fw(r), fw(w), fw(hh), fw(size)
        );
    }
    format!(
        "Look {{ rings: &[{rings}], fill: {fill}, grips: {grips}, scallop: {scallop}, dots: {dots}, arc: {arc}, corner: {corner}, segs: {segs}, path: &[{path}], jog: {jog} }}"
    )
}

fn main() {
    println!("cargo:rerun-if-changed={SRC}");
    let text = fs::read_to_string(SRC).unwrap_or_else(|e| panic!("{SRC}: {e}"));
    let mut out = String::from("// Generated by build.rs from layout/xdj-az.tsv. Do not edit.\n\n");
    for (n, line) in text.lines().enumerate() {
        let at = format!("{SRC}:{}", n + 1);
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        assert!(
            f.len() == 6 || f.len() == 7,
            "{at}: expected 6 or 7 tab-separated fields"
        );
        let ident = f[0].replace('.', "_").to_uppercase();
        match f[1] {
            "offset" => {
                writeln!(out, "pub const {ident}: f32 = {};", fw(num(f[2], &at))).unwrap();
            }
            "circle" | "rect" => {
                let look = look(f.get(6).copied().unwrap_or(""), &at);
                writeln!(out, "static LOOK_{ident}: Look = {look};").unwrap();
                writeln!(
                    out,
                    "pub const {ident}: Geom = Geom {{ u: {:.6}, v: {:.6}, hw: {}, hh: {}, round: {}, look: &LOOK_{ident} }};",
                    num(f[2], &at) / PANEL_W,
                    num(f[3], &at) / PANEL_H,
                    fw(num(f[4], &at) / 2.0),
                    fw(num(f[5], &at) / 2.0),
                    f[1] == "circle"
                )
                .unwrap();
            }
            other => panic!("{at}: unknown shape {other:?}"),
        }
    }
    let dest = Path::new(&env::var("OUT_DIR").unwrap()).join("layout.rs");
    fs::write(dest, out).unwrap();
}
