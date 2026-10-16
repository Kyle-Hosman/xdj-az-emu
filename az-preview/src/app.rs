// SPDX-License-Identifier: GPL-3.0-only
//! The window: the live screen, either alone or inside the full front panel,
//! and the mouse handling that turns clicks and drags into bus frames.
//!
//! Every update composes the whole erp and mixer frames from the app's state
//! (held buttons, latches, analog values) and sends a frame only when it
//! changed, so several controls can be held at once and moving one never
//! resets another.

use crate::boot::{self, Launched};
use crate::config::{knob_ctr_load, knob_ctr_store, scratch};
use crate::controls::{
    append_hex, elapsed_ms, get10, get_mx, gray4, initial_mixer_frame, jog_into, parse_levels,
    parse_pad_leds, set10, set_mx, tempo_into, tempo_raw, xpad_zone_bits, Bit, Bus, CtrlMsg, Frame,
    JogDisplay, JogEvent, LedWire, WaveColumn, BUS_DECK_L, BUS_DECK_R, BUS_ERP, BUS_MIXER,
    DECK_ERP, DECK_TRANSPORT, ERP_DECK2_STRIDE, ERP_IDLE, ERP_SHIFT, FX_ON_BTN, FX_SELECT_KNOB,
    FX_TIME_BYTES, FX_TIME_KNOB, GLOBAL_ERP, JOG_STOPPED, JOG_TOUCH_BACK, JOG_TOUCH_FWD,
    JOG_TURNING, KNOB_BYTE, KNOB_PUSH, LATCH_BYTE, LED_FRAME_SIZE, MIN_HOLD, MIXER_BUTTONS,
    MIXER_SETTINGS, MX10, MX_CH3_FADER, MX_CH4_FADER, MX_COUNT, MX_CROSSFADER, MX_FXSEL_BYTE,
    MX_MASTER_LEVEL, MX_SCFX_PARAM, PANEL_BUTTONS, SPI2_IDLE, SPI3_IDLE, SPI5_IDLE, XPAD_POS_BYTE,
    XPAD_ZONE_BYTE,
};
use crate::layout::{
    Geom, DECK_JOG, DECK_SHIFT, DECK_TEMPO_TRACK, MASTER_LEVEL, MIXER_FADER_3, MIXER_FADER_4,
    MIXER_SCFX_PARAM, MIXER_SETTINGS_BTN, MIXER_XFADER, PANEL_TOWER, TOP_MENU,
};
use crate::meter::{Meter, FLOOR_DB};
use crate::panel::{
    self, browse_center, fit_rect, knob, lcd_rect, xpad_rect, DeckView, PanelView, Sc, ASPECT,
    CH_CONTROLS, COL_BLACK, COL_SILVER, COL_TEAL, PANEL_MARGIN,
};
use crate::screen::{EmuCmd, LCD_H, LCD_W};
use egui::{Color32, Id, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// Drag rotation: ignore points this close to the center (as a fraction of the
/// radius), where a tiny wobble reads as a huge angle.
const MIN_R_FRAC: f32 = 0.25;
/// Drag rotation: the most one frame may turn (radians). A fast flick moves the
/// pointer far between two samples, and the angle between two far points is
/// ambiguous.
const MAX_TURN_PER_FRAME: f32 = 1.2;
/// Browse knob: radians of drag per detent (inverse).
const BROWSE_SENSITIVITY: f32 = 2.2;
/// Jog: position units per radian. A full mouse circle is an eighth of the 16-bit
/// position range, which reads to the deck program like a moderate hand turn.
/// Two SHIFT clicks this close together are a double press.
const SHIFT_DOUBLE: Duration = Duration::from_millis(400);

const JOG_POS_SCALE: f32 = 65536.0 / TAU / 8.0;
/// Mixer knobs: value change per point of vertical drag.
const KNOB_DRAG_GAIN: f32 = 0.004;
/// Beat FX SELECT: points of vertical drag per position.
const FX_PX_PER_STEP: f32 = 14.0;
/// The pop-out button's glyph.
const POP_GLYPH: &str = "\u{29c9}";

/// Channels to the background threads.
pub struct Links {
    pub cmd_tx: Sender<EmuCmd>,
    pub frame_rx: Receiver<egui::ColorImage>,
    pub ctrl_tx: Sender<CtrlMsg>,
    pub led_rx: Receiver<LedWire>,
}

pub struct App {
    links: Links,
    /// The emulator this app started, stopped on exit. None if it attached.
    launched: Option<Launched>,
    /// Shown on the screen instead of the deck's: why this window shows no
    /// emulator (another AZ-EMU has it).
    notice: Option<String>,
    /// `AZ_SHOT`: save a screenshot of the window here and quit.
    shot_path: Option<String>,
    frames_drawn: u32,
    lcd_tex: Option<egui::TextureHandle>,
    /// Show only the screen (resizable) instead of the whole panel.
    screen_only: bool,
    sized_once: bool,

    /// Browse knob: drawn rotation, accumulated drag (detents are derived from
    /// it) and the push flash.
    browse_angle: f32,
    browse_cursor: f32,
    browse_flash: f32,
    /// Per deck, [left, right]: jog rotation, touch, drawn tempo fader.
    jog_angle: [f32; 2],
    jog_touching: [bool; 2],
    tempo_pos: [f32; 2],
    pad_led: [[Color32; 8]; 2],
    /// The deck program's latest erp LED frame (button LEDs, pads).
    erp_leds: [u8; LED_FRAME_SIZE],
    /// Mixer channels 1-4's level meters, each fed by its deck's output, and
    /// when they last fell.
    meters: [Meter; 4],
    meters_at: Instant,
    /// Per deck, [left, right]: the jog display's messages, and the image it
    /// shows (none until a track with artwork is loaded).
    jog_display: [JogDisplay; 2],
    jog_tex: [Option<egui::TextureHandle>; 2],
    /// Per deck, [left, right]: the loaded track's overview waveform, its length
    /// and the elapsed time (ms), as the jog display gets them.
    jog_overview: [Vec<WaveColumn>; 2],
    jog_length: [Option<u32>; 2],
    jog_elapsed: [Option<u32>; 2],
    /// Drawn positions (0..1) of the 8-bit mixer controls, the 10-bit knobs,
    /// the Beat FX SELECT position (0..15) with its partial drag, and the Beat
    /// FX TIME encoder's count with its partial drag.
    mixer_pos: [f32; MX_COUNT],
    mx10_pos: Vec<f32>,
    fx_select: i32,
    fx_drag: f32,
    fx_time: i16,
    fx_time_drag: f32,
    /// X-PAD finger position (0..127) while the strip is pressed.
    xpad: Option<u8>,
    /// SHIFT latch per deck.
    shift: [bool; 2],
    /// When each deck's SHIFT was last clicked, for its double press.
    shift_clicked: [Option<Instant>; 2],
    /// Per deck, the jog display shows the artwork instead of the waveform.
    jog_show_art: [bool; 2],
    /// Buttons held now (by widget id, with when the press began), and short
    /// presses kept down until `MIN_HOLD` has passed.
    held: HashMap<Id, (Bit, Instant)>,
    pending: Vec<(Bit, Instant)>,

    /// The persistent bus state: every mixer level and the knob counter, and
    /// each deck's jog and tempo. Buttons are OR'd in when a frame is composed.
    mixer_frame: Frame,
    deck_frame: [Frame; 2],
    sent_erp: Frame,
    sent_mixer: Frame,
    /// The window title, and when the status files were last read.
    title: String,
    mods_checked: Option<Instant>,
    /// When the first screen frame and the latest erp LED frame arrived: a guest
    /// that draws its screen but sends the panel nothing has no working deck
    /// program.
    screen_since: Option<Instant>,
    erp_at: Option<Instant>,
}

/// How long the screen may run without an erp LED frame before the title says so:
/// the deck program starts some time after the screen does.
const PANEL_SILENT: Duration = Duration::from_secs(90);

/// The title bar carries the warnings, each ending with its fix: the guest's
/// health (tools/az-emu-health.py, which the boot scripts run), the mods
/// installer's when it could not confirm the AZ-MODS set is the newest
/// (tools/az-mods-fetch.py), and `extra`.
fn title_from_status(extra: Option<&str>) -> String {
    let dir = crate::config::sock_dir();
    let mut title = "XDJ-AZ".to_string();
    for file in ["health-status.txt", "mods-status.txt"] {
        if let Ok(s) = std::fs::read_to_string(dir.join(file)) {
            for warn in s.lines().filter_map(|l| l.strip_prefix("WARN ")) {
                title += &format!("  |  {}", warn.trim());
            }
        }
    }
    if let Some(warn) = extra {
        title += &format!("  |  {warn}");
    }
    title
}

impl App {
    pub fn new(
        links: Links,
        launched: Option<Launched>,
        shot_path: Option<String>,
        notice: Option<String>,
    ) -> Self {
        let screen_only = shot_path.is_none() && std::env::var_os("AZ_FULL").is_none();
        App {
            links,
            launched,
            notice,
            shot_path,
            frames_drawn: 0,
            lcd_tex: None,
            screen_only,
            sized_once: false,
            browse_angle: 0.0,
            browse_cursor: 0.0,
            browse_flash: 0.0,
            jog_angle: [0.0; 2],
            jog_touching: [false; 2],
            tempo_pos: [0.5; 2],
            pad_led: [[Color32::BLACK; 8]; 2],
            erp_leds: [0; LED_FRAME_SIZE],
            meters: [Meter::default(); 4],
            meters_at: Instant::now(),
            jog_display: Default::default(),
            jog_tex: [None, None],
            jog_overview: Default::default(),
            jog_length: [None; 2],
            jog_elapsed: [None; 2],
            mixer_pos: std::array::from_fn(|mx| get_mx(&initial_mixer_frame(), mx)),
            mx10_pos: MX10
                .iter()
                .map(|&(_, hi, lo, shift, _)| {
                    f32::from(get10(&initial_mixer_frame(), hi, lo, shift)) / 1023.0
                })
                .collect(),
            fx_select: 1, // the idle frame's SELECT nibble (Gray 1)
            fx_drag: 0.0,
            fx_time: 0,
            fx_time_drag: 0.0,
            xpad: None,
            shift: [false; 2],
            shift_clicked: [None; 2],
            jog_show_art: [false; 2],
            held: HashMap::new(),
            pending: Vec::new(),
            mixer_frame: initial_mixer_frame(),
            deck_frame: [SPI5_IDLE, SPI2_IDLE],
            sent_erp: ERP_IDLE,
            sent_mixer: SPI3_IDLE,
            title: "XDJ-AZ".to_string(),
            mods_checked: None,
            screen_since: None,
            erp_at: None,
        }
    }

    /// Take the newest screen frame and every LED frame from the threads.
    fn drain_links(&mut self, ctx: &egui::Context) {
        if let Some(img) = self.links.frame_rx.try_iter().last() {
            self.lcd_tex = Some(ctx.load_texture("lcd", img, egui::TextureOptions::LINEAR));
            self.screen_since.get_or_insert_with(Instant::now);
        }
        let debug = std::env::var_os("AZ_LED_DEBUG").is_some();
        while let Ok(wire) = self.links.led_rx.try_recv() {
            if debug {
                append_hex(&scratch("az-led-debug.log"), "", &wire);
            }
            if let Some(leds) = parse_pad_leds(&wire) {
                self.erp_at = Some(Instant::now());
                self.pad_led = leds;
                self.erp_leds.copy_from_slice(&wire[1..1 + LED_FRAME_SIZE]);
            } else if let Some(levels) = parse_levels(&wire) {
                for (d, m) in self.meters.iter_mut().enumerate() {
                    m.hit(levels.peak[2 * d].max(levels.peak[2 * d + 1]));
                }
            } else if let Some(d) = [BUS_DECK_L, BUS_DECK_R].iter().position(|&b| b == wire[0]) {
                let frame = &wire[1..1 + LED_FRAME_SIZE];
                if let Some(t) = elapsed_ms(frame, d) {
                    self.jog_elapsed[d] = Some(t);
                }
                match self.jog_display[d].feed(frame) {
                    Some(JogEvent::Load(length)) => {
                        self.jog_tex[d] = None;
                        self.jog_overview[d].clear();
                        self.jog_length[d] = (length > 0).then_some(length);
                    }
                    Some(JogEvent::Overview(cols)) => self.jog_overview[d] = cols,
                    Some(JogEvent::Art(jpeg)) => {
                        self.jog_tex[d] = decode_jpeg(&jpeg).map(|img| {
                            ctx.load_texture(format!("jog{d}"), img, egui::TextureOptions::LINEAR)
                        })
                    }
                    None => {}
                }
            }
        }
        let dt = self.meters_at.elapsed().as_secs_f32();
        self.meters_at = Instant::now();
        for m in &mut self.meters {
            m.fall(dt);
        }
    }

    /// Resize the window to the screen at 1:1 physical pixels (+ a margin).
    fn size_for_screen(ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let size = egui::vec2(LCD_W / ppp + 12.0, LCD_H / ppp + 12.0);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    /// Resize the window for the whole panel, inside the monitor and clear of a
    /// taskbar or dock along its bottom. (The viewport's clamp-to-monitor
    /// setting does not cover a later InnerSize.)
    fn size_for_panel(ctx: &egui::Context) {
        let (mw, mh) = ctx
            .input(|i| i.viewport().monitor_size)
            .map_or((1600.0, 1600.0 / ASPECT + 100.0), |m| (m.x, m.y));
        // egui gives the whole monitor, not the part a taskbar leaves free:
        // this much is kept below the window for one (Windows' is 48 points).
        const BAR: f32 = 56.0;
        // Room for the title bar too, before the window exists to measure it.
        let w = (mw - 60.0)
            .min((mh - BAR - 60.0) * ASPECT)
            .clamp(400.0, 1600.0);
        let h = (w - 2.0 * PANEL_MARGIN) / ASPECT + 2.0 * PANEL_MARGIN;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
        // The window grows from where the system first put it (for the smaller
        // screen-only size), which can push its bottom and right off the
        // monitor: then it is moved back on. Only for the primary monitor (the
        // one at the origin): egui gives the monitor's size, not its place.
        let rects = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        if let (Some(outer), Some(inner)) = rects {
            let deco = outer.size() - inner.size();
            let on_primary =
                outer.min.x >= 0.0 && outer.min.y >= 0.0 && outer.min.x < mw && outer.min.y < mh;
            let x = outer.min.x.min(mw - w - deco.x).max(0.0);
            let y = outer.min.y.min(mh - BAR - h - deco.y).max(0.0);
            if on_primary && (x, y) != (outer.min.x, outer.min.y) {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            }
        }
    }

    /// The live screen into `rect`, or why there is none yet; a click there is
    /// a tap on the deck's touch screen.
    fn screen(&mut self, ui: &Ui, rect: Rect, id: &str) {
        if let Some(tex) = &self.lcd_tex {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
        } else {
            // In both views: why there is no picture. The panel view otherwise
            // keeps its printed placeholder while the guest boots.
            let msg = match (&self.notice, boot::failure(&mut self.launched)) {
                (Some(n), _) => Some(n.clone()),
                (None, Some(why)) => {
                    // The boot script's log on macOS; on Windows the script's own
                    // error line is the message.
                    let log = if cfg!(unix) {
                        format!(
                            "\n(full log: {})",
                            crate::config::sock_dir().join("qemu.log").display()
                        )
                    } else {
                        String::new()
                    };
                    Some(format!("the emulator did not start:\n{why}{log}"))
                }
                (None, None) => self
                    .screen_only
                    .then(|| "waiting for guest\u{2026}".to_string()),
            };
            if let Some(msg) = msg {
                ui.painter().rect_filled(rect, 0.0, COL_BLACK);
                // Wrapped to the screen: a path in the message can be long.
                let font = egui::FontId::proportional(16.0);
                let text = ui
                    .painter()
                    .layout(msg, font, COL_SILVER, rect.width() * 0.9);
                let at = rect.center() - text.size() * 0.5;
                ui.painter().galley(at, text, COL_SILVER);
            }
        }
        let resp = ui.interact(rect, ui.id().with(id), Sense::click());
        if let (true, Some(pos)) = (resp.clicked(), resp.interact_pointer_pos()) {
            let u = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let v = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            let _ = self.links.cmd_tx.send(EmuCmd::Tap(u, v));
        }
    }

    /// Only the screen, scaled down to fit but never up past 1:1 physical pixels.
    fn screen_only_view(&mut self, ui: &mut Ui, area: Rect) {
        ui.painter().rect_filled(area, 0.0, COL_BLACK);
        let mut lcd = fit_rect(area.shrink(6.0), LCD_W / LCD_H);
        let ppp = ui.ctx().pixels_per_point();
        if lcd.width() > LCD_W / ppp {
            lcd = Rect::from_center_size(lcd.center(), Vec2::new(LCD_W / ppp, LCD_H / ppp));
        }
        self.screen(ui, lcd, "lcd-touch-so");
        let bb = Rect::from_min_size(
            Pos2::new(area.right() - 34.0, area.top() + 8.0),
            Vec2::new(26.0, 20.0),
        );
        let back = ui
            .put(bb, egui::Button::new(POP_GLYPH).small())
            .on_hover_text("Back to controller");
        if back.clicked() {
            self.screen_only = false;
            Self::size_for_panel(ui.ctx());
        }
    }

    /// The whole panel and every control on it. Order matters where hit areas
    /// overlap: egui gives a click to the widget registered last, so the
    /// buttons come after the jogs, whose square hit areas cover BEAT LOOP and
    /// BEAT JUMP.
    fn panel_view(&mut self, ui: &mut Ui, area: Rect) {
        self.browse_flash = (self.browse_flash - 0.12).max(0.0);
        let decks = [0, 1].map(|d| DeckView {
            tempo: self.tempo_pos[d],
            jog_angle: self.jog_angle[d],
            pad_led: self.pad_led[d],
            art: self.jog_tex[d].as_ref().map(|t| t.id()),
            overview: &self.jog_overview[d],
            position: self.jog_length[d]
                .zip(self.jog_elapsed[d])
                .filter(|&(len, _)| len > 0)
                .map(|(len, t)| (t as f32 / len as f32).clamp(0.0, 1.0)),
            show_art: self.jog_show_art[d],
        });
        let view = PanelView {
            decks,
            mixer: &self.mixer_pos,
            browse_angle: self.browse_angle,
            browse_flash: self.browse_flash,
            erp: &self.erp_leds,
            meters: &self.meters,
        };
        panel::paint(ui.painter(), area, &view);
        let sc = Sc::new(area);
        self.browse_knob(ui, &sc);
        self.jogs(ui, &sc);
        self.tempo_faders(ui, &sc);
        self.mixer_controls(ui, &sc);
        self.buttons(ui, &sc);
        self.shift_latches(ui, &sc);
        self.mx10_knobs(ui, &sc);
        self.fx_select(ui, &sc);
        self.fx_time(ui, &sc);
        self.xpad_strip(ui, &sc);

        let lcd = lcd_rect(&sc);
        self.screen(ui, lcd, "lcd-touch");
        // In the screen housing's top-right corner, level with the screen tabs.
        let tower = sc.g(PANEL_TOWER);
        let tb = Rect::from_center_size(
            Pos2::new(tower.right() - sc.lr(0.012), sc.c(TOP_MENU).y),
            Vec2::new(26.0, 20.0),
        );
        if ui
            .put(tb, egui::Button::new(POP_GLYPH).small())
            .on_hover_text("Pop out screen only")
            .clicked()
        {
            self.screen_only = true;
            Self::size_for_screen(ui.ctx());
        }
    }

    /// The browse knob: drag anywhere on it to turn, click the cap to push.
    /// egui tells a stationary click from a drag, so the two never collide.
    fn browse_knob(&mut self, ui: &Ui, sc: &Sc) {
        let (c, r) = browse_center(sc);
        let resp = ui.interact(
            Rect::from_center_size(c, Vec2::splat(r * 2.0)),
            ui.id().with("browse-knob"),
            Sense::click_and_drag(),
        );
        if let Some(d) = drag_turn(&resp, c, r) {
            self.browse_angle += d;
            let prev_step = (self.browse_cursor * BROWSE_SENSITIVITY) as i32;
            self.browse_cursor += d;
            let step = (self.browse_cursor * BROWSE_SENSITIVITY) as i32;
            if step != prev_step {
                // Continue from the shared counter: az-drive.py may have turned
                // the knob since, and two private counters make phantom detents.
                // Stop at the ends rather than wrap (see controls::SPI3_IDLE).
                let base = knob_ctr_load().unwrap_or(self.mixer_frame[KNOB_BYTE]);
                let next = (i32::from(base) + step - prev_step).clamp(1, 254) as u8;
                knob_ctr_store(next);
                self.mixer_frame[KNOB_BYTE] = next;
            }
        }
        if let (true, Some(pos)) = (resp.clicked(), resp.interact_pointer_pos()) {
            if (pos - c).length() <= r * 0.55 {
                self.browse_flash = 1.0;
                // A pulse held for MIN_HOLD: the guest polls the mixer bus too
                // slowly to see a press and release sent back to back.
                let (byte, mask) = KNOB_PUSH;
                self.pending
                    .push((Bit::panel(byte, mask), Instant::now() + MIN_HOLD));
            }
        }
    }

    /// Jog wheels: a drag turns the platter (touched), release keeps the
    /// position (a jump back would read as a fast spin) and clears the touch.
    fn jogs(&mut self, ui: &Ui, sc: &Sc) {
        for d in 0..2 {
            let jog = DECK_JOG.on_deck(d);
            let (c, r) = (sc.c(jog), sc.r(jog));
            let resp = ui.interact(
                Rect::from_center_size(c, Vec2::splat(r * 2.0)),
                ui.id().with(("jog", d)),
                Sense::click_and_drag(),
            );
            // The hit area is the platter's square: a drag only counts when it
            // started on the platter itself, not on a button in a corner
            // (LOOP 8, TRACK SEARCH, SEARCH, JOG FEEL).
            let on_platter = ui
                .ctx()
                .input(|i| i.pointer.press_origin())
                .is_some_and(|p| (p - c).length() <= r);
            let dragging = resp.dragged() && on_platter;
            let turn = drag_turn(&resp, c, r).filter(|_| on_platter);
            if let Some(t) = turn {
                self.jog_angle[d] += t;
            }
            if turn.is_some() || (!dragging && self.jog_touching[d]) {
                let pos = (self.jog_angle[d] * JOG_POS_SCALE).rem_euclid(65536.0) as u16;
                // Direction rides the touch byte, as on the deck; the period says the
                // platter is moving, and the release says it stopped (never 0: see
                // JOG_STOPPED).
                let (touch, period) = jog_touch(turn, dragging);
                jog_into(&mut self.deck_frame[d], touch, pos, period);
                self.send_deck(d);
            }
            self.jog_touching[d] = dragging;
        }
    }

    /// Tempo faders: the pointer's position along the fader.
    fn tempo_faders(&mut self, ui: &Ui, sc: &Sc) {
        for d in 0..2 {
            if let Some(t) = vertical_fader(ui, sc, DECK_TEMPO_TRACK.on_deck(d), ("tempo", d)) {
                self.tempo_pos[d] = t;
                tempo_into(&mut self.deck_frame[d], tempo_raw(t));
                self.send_deck(d);
            }
        }
    }

    fn send_deck(&self, d: usize) {
        let bus = if d == 0 { BUS_DECK_L } else { BUS_DECK_R };
        let _ = self.links.ctrl_tx.send(CtrlMsg {
            bus,
            frame: self.deck_frame[d],
        });
    }

    /// The mixer controls in `MX_AT`: knobs follow vertical drag, faders and
    /// the crossfader follow the pointer.
    fn mixer_controls(&mut self, ui: &Ui, sc: &Sc) {
        fn set(app: &mut App, mx: usize, t: f32) {
            app.mixer_pos[mx] = t;
            set_mx(&mut app.mixer_frame, mx, t);
        }
        let mut knobs: Vec<(Geom, usize)> = vec![
            (MIXER_SCFX_PARAM, MX_SCFX_PARAM),
            (MASTER_LEVEL, MX_MASTER_LEVEL),
        ];
        for ch in &CH_CONTROLS {
            knobs.extend(ch[..5].iter().copied());
        }
        for (i, &(g, mx)) in knobs.iter().enumerate() {
            let hit = Rect::from_center_size(sc.c(g), Vec2::splat(sc.r(g) * 2.4));
            let resp = ui.interact(hit, ui.id().with(("mxknob", i)), Sense::drag());
            if resp.dragged() {
                let t = (self.mixer_pos[mx] - resp.drag_delta().y * KNOB_DRAG_GAIN).clamp(0.0, 1.0);
                set(self, mx, t);
            }
        }
        let faders = CH_CONTROLS
            .iter()
            .map(|ch| ch[5])
            .chain([(MIXER_FADER_3, MX_CH3_FADER), (MIXER_FADER_4, MX_CH4_FADER)]);
        for (i, (g, mx)) in faders.enumerate() {
            if let Some(t) = vertical_fader(ui, sc, g, ("mxfader", i)) {
                set(self, mx, t);
            }
        }
        let (ul, ur) = MIXER_XFADER.travel_u();
        let (l, r, cy) = (sc.p(ul, 0.0).x, sc.p(ur, 0.0).x, sc.c(MIXER_XFADER).y);
        let half = sc.g(crate::layout::MIXER_XFADER_CAP).height() * 0.5;
        let hit = Rect::from_min_max(Pos2::new(l, cy - half), Pos2::new(r, cy + half));
        let resp = ui.interact(hit, ui.id().with("mx-crossfader"), Sense::click_and_drag());
        if let Some(pos) = pressed_at(&resp) {
            set(self, MX_CROSSFADER, ((pos.x - l) / (r - l)).clamp(0.0, 1.0));
        }
    }

    /// Every momentary button: one list of (rect, bit, id, name), then one
    /// press/release pass. A held button's bit is OR'd into the composed frame;
    /// a release sooner than MIN_HOLD is deferred until it has passed.
    fn buttons(&mut self, ui: &Ui, sc: &Sc) {
        let mut list: Vec<(Geom, Bit, Id, &str)> = Vec::new();
        for (i, b) in PANEL_BUTTONS.iter().enumerate() {
            list.push((
                b.g,
                Bit::panel(b.byte, b.mask),
                ui.id().with(("panelbtn", i)),
                b.name,
            ));
        }
        for d in 0..2 {
            for (i, b) in DECK_TRANSPORT.iter().chain(DECK_ERP).enumerate() {
                let (byte, mask) = if d == 0 {
                    b.bit_l
                } else {
                    (b.bit_r.0 + ERP_DECK2_STRIDE, b.bit_r.1)
                };
                list.push((
                    b.g.on_deck(d),
                    Bit::erp(byte, mask),
                    ui.id().with(("deckbtn", d, i)),
                    b.name,
                ));
            }
        }
        for (i, b) in GLOBAL_ERP.iter().enumerate() {
            list.push((
                b.g,
                Bit::erp(b.byte, b.mask),
                ui.id().with(("globalbtn", i)),
                b.name,
            ));
        }
        let (byte, mask) = MIXER_SETTINGS;
        list.push((
            MIXER_SETTINGS_BTN,
            Bit::panel(byte, mask),
            ui.id().with("mixer-settings"),
            "MIXER SETTINGS",
        ));
        let fx_on = Bit {
            bus: Bus::Mixer,
            byte: MX_FXSEL_BYTE,
            mask: 0x01,
            latch: false,
        };
        list.push((FX_ON_BTN, fx_on, ui.id().with("fx-on"), "BEAT FX ON/OFF"));
        for (i, b) in MIXER_BUTTONS.iter().enumerate() {
            let bit = Bit {
                bus: Bus::Mixer,
                byte: b.byte,
                mask: b.mask,
                latch: false,
            };
            list.push((b.g, bit, ui.id().with(("mixerbtn", i)), b.name));
        }

        // AZ_SHOW_HITS=1 outlines every hit area (for layout work).
        let show_hits = std::env::var_os("AZ_SHOW_HITS").is_some();
        let now = Instant::now();
        for (geom, bit, id, name) in list {
            let rect = sc.g(geom);
            // The bounding box takes the click; the button's own outline decides
            // (the curved keys and round buttons do not fill their box).
            let resp = ui.interact(rect, id, Sense::click()).on_hover_text(name);
            let on_shape = resp.interact_pointer_pos().is_some_and(|q| {
                let (u, v) = sc.uv(q);
                geom.contains(u, v)
            });
            let down = resp.is_pointer_button_down_on() && on_shape;
            if down && !self.held.contains_key(&id) {
                self.held.insert(id, (bit, now));
            } else if !down {
                if let Some((bit, since)) = self.held.remove(&id) {
                    if now < since + MIN_HOLD {
                        self.pending.push((bit, since + MIN_HOLD));
                    }
                }
            }
            if down {
                highlight(ui, sc, rect, 0.002, 1.0);
            }
            if show_hits {
                ui.painter()
                    .rect_stroke(rect, 0.0, Stroke::new(1.0_f32, Color32::RED));
            }
        }
    }

    /// SHIFT is a modifier: a click latches it until the next click, and its
    /// bit rides in every erp frame meanwhile.
    fn shift_latches(&mut self, ui: &Ui, sc: &Sc) {
        for d in 0..2 {
            let rect = sc.g(DECK_SHIFT.on_deck(d));
            let resp = ui.interact(rect, ui.id().with(("shift", d)), Sense::click());
            if resp
                .on_hover_text("SHIFT (click to latch; double-click: jog display art/waveform)")
                .clicked()
            {
                self.shift[d] = !self.shift[d];
                // A double press switches the jog display between the artwork and
                // the waveform; the two clicks leave the latch as it was.
                let now = Instant::now();
                if self.shift_clicked[d].is_some_and(|t| now - t <= SHIFT_DOUBLE) {
                    self.jog_show_art[d] = !self.jog_show_art[d];
                    self.shift_clicked[d] = None;
                } else {
                    self.shift_clicked[d] = Some(now);
                }
            }
            if self.shift[d] {
                highlight(ui, sc, rect, 0.003, 2.0);
            }
        }
    }

    /// The 10-bit knobs (channel 3/4 trim and EQ, MIC 2 EQ), drawn live over
    /// the panel's static ones.
    fn mx10_knobs(&mut self, ui: &Ui, sc: &Sc) {
        for (i, &(g, hi, lo, shift, name)) in MX10.iter().enumerate() {
            let hit = Rect::from_center_size(sc.c(g), Vec2::splat(sc.r(g) * 2.4));
            let resp = ui
                .interact(hit, ui.id().with(("mx10", i)), Sense::drag())
                .on_hover_text(name);
            if resp.dragged() {
                self.mx10_pos[i] =
                    (self.mx10_pos[i] - resp.drag_delta().y * KNOB_DRAG_GAIN).clamp(0.0, 1.0);
                set10(
                    &mut self.mixer_frame,
                    hi,
                    lo,
                    shift,
                    (self.mx10_pos[i] * 1023.0).round() as u16,
                );
            }
            knob(ui.painter(), sc, g, Some(self.mx10_pos[i]));
        }
    }

    /// Beat FX SELECT: 16 positions, one per FX_PX_PER_STEP of vertical drag.
    fn fx_select(&mut self, ui: &Ui, sc: &Sc) {
        let g = FX_SELECT_KNOB;
        let hit = Rect::from_center_size(sc.c(g), Vec2::splat(sc.r(g) * 2.2));
        let resp = ui.interact(hit, ui.id().with("fx-select"), Sense::drag());
        if resp.dragged() {
            self.fx_drag -= resp.drag_delta().y;
            let steps = (self.fx_drag / FX_PX_PER_STEP).trunc();
            self.fx_drag -= steps * FX_PX_PER_STEP;
            self.fx_select = (self.fx_select + steps as i32).rem_euclid(16);
            let f = &mut self.mixer_frame[MX_FXSEL_BYTE];
            *f = (gray4(self.fx_select) << 4) | (*f & 0x0f);
        }
        knob(ui.painter(), sc, g, Some(self.fx_select as f32 / 15.0));
    }

    /// Beat FX TIME: an endless encoder, one count per FX_PX_PER_STEP of
    /// vertical drag (up is clockwise). The pointer shows the count's last 16
    /// steps, as the knob has no end to show.
    fn fx_time(&mut self, ui: &Ui, sc: &Sc) {
        let g = FX_TIME_KNOB;
        let hit = Rect::from_center_size(sc.c(g), Vec2::splat(sc.r(g) * 2.2));
        let resp = ui
            .interact(hit, ui.id().with("fx-time"), Sense::drag())
            .on_hover_text("BEAT FX TIME (drag up/down)");
        if resp.dragged() {
            self.fx_time_drag -= resp.drag_delta().y;
            let steps = (self.fx_time_drag / FX_PX_PER_STEP).trunc();
            self.fx_time_drag -= steps * FX_PX_PER_STEP;
            self.fx_time = self.fx_time.wrapping_add(steps as i16);
            let [lo, hi] = self.fx_time.to_le_bytes();
            self.mixer_frame[FX_TIME_BYTES.0] = lo;
            self.mixer_frame[FX_TIME_BYTES.1] = hi;
        }
        let shown = f32::from(i16::rem_euclid(self.fx_time, 16)) / 15.0;
        knob(ui.painter(), sc, g, Some(shown));
    }

    /// The X-PAD strip: press or drag anywhere on it; release lifts the finger.
    fn xpad_strip(&mut self, ui: &Ui, sc: &Sc) {
        let strip = xpad_rect(sc);
        let resp = ui
            .interact(
                strip.expand(sc.l(0.006)),
                ui.id().with("xpad"),
                Sense::click_and_drag(),
            )
            .on_hover_text("X-PAD (press / drag)");
        self.xpad = resp
            .interact_pointer_pos()
            .filter(|_| resp.is_pointer_button_down_on())
            .map(|p| {
                (((p.x - strip.left()) / strip.width()).clamp(0.0, 1.0) * 127.0).round() as u8
            });
        if let Some(x) = self.xpad {
            let px = strip.left() + strip.width() * (f32::from(x) / 127.0);
            let line = [Pos2::new(px, strip.top()), Pos2::new(px, strip.bottom())];
            ui.painter()
                .line_segment(line, Stroke::new(sc.l(0.003).max(2.0), COL_TEAL));
        }
    }

    /// Compose the erp and mixer frames and send whichever changed.
    fn send_frames(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.pending.retain(|(_, until)| *until > now);
        // Adopt a knob counter az-drive.py moved, without sending a frame for
        // that alone: the guest already has it.
        if let Some(k) = knob_ctr_load() {
            if k != self.mixer_frame[KNOB_BYTE] {
                self.mixer_frame[KNOB_BYTE] = k;
                self.sent_mixer[KNOB_BYTE] = k;
            }
        }
        let mut erp = ERP_IDLE;
        let mut mix = self.mixer_frame;
        let mut latch = false;
        for bit in self
            .held
            .values()
            .map(|(b, _)| *b)
            .chain(self.pending.iter().map(|(b, _)| *b))
        {
            match bit.bus {
                Bus::Erp => erp[bit.byte] |= bit.mask,
                Bus::Mixer => {
                    mix[bit.byte] |= bit.mask;
                    latch |= bit.latch;
                }
            }
        }
        for (d, &on) in self.shift.iter().enumerate() {
            if on {
                erp[ERP_SHIFT.0 + d * ERP_DECK2_STRIDE] |= ERP_SHIFT.1;
            }
        }
        if let Some(x) = self.xpad {
            mix[XPAD_POS_BYTE] = x & 0x7f;
            mix[XPAD_ZONE_BYTE] = xpad_zone_bits(x);
            latch = true;
        }
        if latch {
            mix[LATCH_BYTE] = 0x00;
        }
        if erp != self.sent_erp {
            let _ = self.links.ctrl_tx.send(CtrlMsg {
                bus: BUS_ERP,
                frame: erp,
            });
            self.sent_erp = erp;
        }
        if mix != self.sent_mixer {
            let _ = self.links.ctrl_tx.send(CtrlMsg {
                bus: BUS_MIXER,
                frame: mix,
            });
            self.sent_mixer = mix;
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(15));
        }
    }

    /// `AZ_SHOT`: on the third frame, screenshot the window to a PNG and quit.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.shot_path.clone() else {
            return;
        };
        self.frames_drawn += 1;
        if self.frames_drawn == 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(img) = shot else { return };
        let (w, h) = (img.size[0] as u32, img.size[1] as u32);
        let buf: Vec<u8> = img.pixels.iter().flat_map(|px| px.to_array()).collect();
        match image::RgbaImage::from_raw(w, h, buf).map(|b| b.save(&path)) {
            Some(Ok(())) => eprintln!("saved screenshot {w}x{h} -> {path}"),
            Some(Err(e)) => eprintln!("could not save {path}: {e}"),
            None => eprintln!("could not save {path}: bad image size"),
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(launched) = &mut self.launched {
            // QEMU quits on its own first (flushing its disk caches to the image
            // files); stop() waits for that and kills it only if it hangs.
            let _ = self.links.cmd_tx.send(EmuCmd::Quit);
            boot::stop(launched);
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // pixels_per_point is unknown before the window exists, so the exact
        // startup size is set here, once.
        if !self.sized_once {
            self.sized_once = true;
            if self.screen_only {
                Self::size_for_screen(ctx);
            } else if self.shot_path.is_none() {
                Self::size_for_panel(ctx);
            }
        }
        self.drain_links(ctx);
        if self
            .mods_checked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.mods_checked = Some(Instant::now());
            let silent = self
                .screen_since
                .is_some_and(|t| t.elapsed() > PANEL_SILENT)
                && self.erp_at.is_none_or(|t| t.elapsed() > PANEL_SILENT);
            let title = title_from_status(silent.then_some(
                "the deck program is not driving the panel: quit AZ-EMU and open it again",
            ));
            if title != self.title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.title = title;
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(COL_BLACK))
            .show(ctx, |ui| {
                let area = ui.max_rect();
                if self.screen_only {
                    self.screen_only_view(ui, area);
                } else {
                    self.panel_view(ui, area);
                }
            });
        // Every view sends the panel state, so the guest gets the starting mixer
        // positions even when only the screen is shown.
        self.send_frames(ctx);
        self.screenshot(ctx);
        // Meters move smoothly only while redrawn often; idle, slower is enough.
        let moving = self.meters.iter().any(|m| m.db > FLOOR_DB);
        ctx.request_repaint_after(Duration::from_millis(if moving { 30 } else { 80 }));
    }
}

/// A JPEG as an egui image, or None if it does not decode.
fn decode_jpeg(jpeg: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .ok()?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// One frame's turn (radians) of a drag around `c` (radius `r`), or None when
/// not dragging or too close to the center to tell.
fn drag_turn(resp: &Response, c: Pos2, r: f32) -> Option<f32> {
    if !resp.dragged() {
        return None;
    }
    let pos = resp.interact_pointer_pos()?;
    let prev = pos - resp.drag_delta();
    if (pos - c).length() < r * MIN_R_FRAC || (prev - c).length() < r * MIN_R_FRAC {
        return None;
    }
    let d = ((pos - c).angle() - (prev - c).angle() + PI).rem_euclid(TAU) - PI;
    Some(d.clamp(-MAX_TURN_PER_FRAME, MAX_TURN_PER_FRAME))
}

/// The jog frame's touch byte and period for one drawn frame of a platter drag.
/// Turning sends the direction with the turning period. A touched platter that
/// did not move this frame (the window repaints between pointer moves) is held
/// still: forward touch with the stopped period. Counting such a frame as a
/// forward turn made a backward drag creep forward. Released: no touch, stopped.
fn jog_touch(turn: Option<f32>, dragging: bool) -> (u8, u16) {
    match turn {
        Some(t) if dragging && t != 0.0 => (
            if t < 0.0 {
                JOG_TOUCH_BACK
            } else {
                JOG_TOUCH_FWD
            },
            JOG_TURNING,
        ),
        _ => (if dragging { JOG_TOUCH_FWD } else { 0 }, JOG_STOPPED),
    }
}

/// Where a press or drag is, if there is one.
fn pressed_at(resp: &Response) -> Option<Pos2> {
    (resp.dragged() || resp.clicked())
        .then(|| resp.interact_pointer_pos())
        .flatten()
}

/// A vertical fader's hit area at column `u` between `v_top` and `v_bot`; the
/// pressed position, 0 = bottom, 1 = top.
fn vertical_fader(ui: &Ui, sc: &Sc, track: Geom, id: impl std::hash::Hash) -> Option<f32> {
    let (v_top, v_bot) = track.travel_v();
    let (top, bot, cx) = (sc.p(0.0, v_top).y, sc.p(0.0, v_bot).y, sc.c(track).x);
    // As wide as the widest cap, so a grab on the cap's edge still moves it.
    let half = sc.lr(crate::layout::DECK_TEMPO_CAP.hw);
    let hit = Rect::from_min_max(Pos2::new(cx - half, top), Pos2::new(cx + half, bot));
    let resp = ui.interact(hit, ui.id().with(id), Sense::click_and_drag());
    pressed_at(&resp).map(|pos| ((pos.y - bot) / (top - bot)).clamp(0.0, 1.0))
}

/// Outline a pressed or latched control; `width` is a control size, `min_px`
/// keeps it visible in a small window.
fn highlight(ui: &Ui, sc: &Sc, rect: Rect, width: f32, min_px: f32) {
    ui.painter().rect_stroke(
        rect,
        sc.l(0.002),
        Stroke::new(sc.l(width).max(min_px), COL_TEAL),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_wrap_takes_the_short_way() {
        // The same expression drag_turn uses, across the -PI/PI seam.
        let wrap = |a: f32, b: f32| (a - b + PI).rem_euclid(TAU) - PI;
        assert!((wrap(PI - 0.1, -PI + 0.1) - (-0.2)).abs() < 1e-5);
        assert!((wrap(-PI + 0.1, PI - 0.1) - 0.2).abs() < 1e-5);
        assert!((wrap(0.3, 0.1) - 0.2).abs() < 1e-5);
    }

    #[test]
    fn a_still_touched_platter_is_held_not_turned() {
        assert_eq!(jog_touch(Some(-0.05), true), (JOG_TOUCH_BACK, JOG_TURNING));
        assert_eq!(jog_touch(Some(0.05), true), (JOG_TOUCH_FWD, JOG_TURNING));
        // no movement this frame while touched: held, never a forward turn
        assert_eq!(jog_touch(Some(0.0), true), (JOG_TOUCH_FWD, JOG_STOPPED));
        assert_eq!(jog_touch(None, true), (JOG_TOUCH_FWD, JOG_STOPPED));
        // released
        assert_eq!(jog_touch(None, false), (0, JOG_STOPPED));
        assert_eq!(jog_touch(Some(0.0), false), (0, JOG_STOPPED));
    }
}
