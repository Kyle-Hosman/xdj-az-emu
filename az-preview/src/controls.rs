// SPDX-License-Identifier: GPL-3.0-only
//! The front panel's wire format: the idle frame of each MCU bus, where every
//! button, knob and fader lives in those frames, and the socket that carries
//! them to the guest.
//!
//! The guest's `subucom_virt` module keeps one overlay frame per bus and serves
//! it to the deck program in place of the real MCU. A control frame on
//! `ctrl.sock` is `[bus id] + 152 bytes`; the module re-stamps each bus's CRC,
//! so the CRC bytes in the templates below are never trusted. The same socket
//! carries the pad LED frames the deck program writes back.
//!
//! Every layout here was measured on a real deck (bus captures while moving one
//! control at a time) and checked on the emulator against the deck program's
//! screen or LEDs.

use crate::config::{mixer_state_store, scratch, sock_dir};
use crate::layout::*;
use egui::Color32;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;
// Windows 10 1803+ has AF_UNIX sockets and QEMU uses them there; std exposes
// them only on unix, uds_windows does on Windows.
#[cfg(windows)]
use uds_windows::UnixStream;

/// A bus frame as the module serves it: 128 real bytes, zero-padded to 152.
pub type Frame = [u8; 152];

// ---- Bus ids --------------------------------------------------------------------

/// spi1, the "erp" bus: both decks' buttons and pads, and the pad LEDs.
pub const BUS_ERP: u8 = 0;
/// spi2: the right deck's jog wheel and tempo fader.
pub const BUS_DECK_R: u8 = 1;
/// spi3: the mixer, the panel buttons and the browse knob.
pub const BUS_MIXER: u8 = 2;
/// spi5: the left deck's jog wheel and tempo fader.
pub const BUS_DECK_L: u8 = 3;
/// Not an SPI bus: the deck program's audio levels, measured in the guest by
/// master_out.so and sent with the LED frames (see [`parse_levels`]).
pub const BUS_LEVELS: u8 = 4;

// ---- Idle frames (captured on a real deck) ----------------------------------------

/// Right deck (spi2) at rest.
pub const SPI2_IDLE: Frame = [
    2, 1, 0, 0, 0, 0, 83, 0, 0, 0, 0, 0, 255, 255, 0, 0, 117, 2, 6, 2, 205, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 38, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Left deck (spi5) at rest.
pub const SPI5_IDLE: Frame = [
    2, 1, 0, 0, 0, 0, 68, 0, 0, 0, 104, 0, 255, 255, 0, 0, 70, 2, 5, 2, 194, 0, 105, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 142, 134, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The erp bus at rest. Byte 24 = 0x01 is the event-frame marker that press and
/// release frames share; a button is a bit elsewhere in the frame.
pub const ERP_IDLE: Frame = [
    0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 57, 72, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The mixer bus at rest. Byte 6 is the shared press latch (0x08 idle, 0x00
/// while a panel button is down), byte 32 the screen-tab row, byte 33 the
/// corner and LOAD row.
///
/// Byte 34 is the browse knob's 8-bit counter. The deck program reads a turn as
/// the plain difference between two frames, so crossing 255/0 reads as a huge
/// turn the other way. It starts mid-range, here, in `tools/az-drive.py` and in
/// the guest's idle frame, which leaves 127 detents each way per boot.
pub const SPI3_IDLE: Frame = [
    3, 1, 2, 1, 1, 1, 8, 0, 0, 0, 0, 0, 0, 0, 0, 124, 2, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 255,
    0, 0, 0, 0, 128, 0, 123, 128, 128, 128, 64, 123, 128, 128, 128, 0, 128, 128, 128, 128, 192,
    128, 128, 128, 128, 0, 0, 128, 128, 128, 0, 128, 128, 128, 128, 0, 128, 128, 128, 42, 2, 255,
    0, 128, 0, 192, 128, 121, 128, 0, 0, 0, 0, 0, 1, 0, 10, 0, 176, 4, 244, 1, 0, 88, 0, 0, 82, 17,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Mixer byte of the browse knob counter (see [`SPI3_IDLE`]).
pub const KNOB_BYTE: usize = 34;
/// Mixer byte of the browse knob push (bit 0x01).
pub const KNOB_PUSH: (usize, u8) = (31, 0x01);
/// Mixer byte of the shared press latch.
pub const LATCH_BYTE: usize = 6;
/// MIXER SETTINGS (the gear): opens the deck program's MIXER SETTINGS screen,
/// and a second press closes it.
pub const MIXER_SETTINGS: (usize, u8) = (0x18, 0x10);
/// X-PAD touch strip: a 7-bit finger position and a zone bitmap (non-zero while
/// touched).
pub const XPAD_POS_BYTE: usize = 0x1d;
pub const XPAD_ZONE_BYTE: usize = 0x1e;

// ---- Buttons ----------------------------------------------------------------------

/// Which bus a momentary button bit rides.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Bus {
    Erp,
    Mixer,
}

/// One momentary button bit. `latch`: also drive the mixer bus's shared press
/// latch, which the panel buttons and the knob push need.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bit {
    pub bus: Bus,
    pub byte: usize,
    pub mask: u8,
    pub latch: bool,
}

impl Bit {
    /// A button on the erp bus.
    pub const fn erp(byte: usize, mask: u8) -> Bit {
        Bit {
            bus: Bus::Erp,
            byte,
            mask,
            latch: false,
        }
    }
    /// A panel button on the mixer bus (sets the press latch).
    pub const fn panel(byte: usize, mask: u8) -> Bit {
        Bit {
            bus: Bus::Mixer,
            byte,
            mask,
            latch: true,
        }
    }
}

/// A button drawn at a fixed place on the panel: where it is, its bus bit, and
/// a name for the hover text.
pub struct PanelButton {
    pub g: Geom,
    pub byte: usize,
    pub mask: u8,
    pub name: &'static str,
}

const fn pb(g: Geom, byte: usize, mask: u8, name: &'static str) -> PanelButton {
    PanelButton {
        g,
        byte,
        mask,
        name,
    }
}

/// The erp bytes of deck 2 are deck 1's plus this stride.
pub const ERP_DECK2_STRIDE: usize = 8;
/// SHIFT (deck 1): a modifier, so the app latches it on click.
pub const ERP_SHIFT: (usize, u8) = (0x09, 0x80);

/// PLAY, CUE, the pad-mode row and the pads, at their place on the left deck:
/// deck-1 byte 10 = PLAY 0x02 / CUE 0x04, byte 13 = pad modes, byte 14 = pads
/// 1-8. The right deck has them at the same place on its own deck, and adds
/// [`ERP_DECK2_STRIDE`].
pub const DECK_TRANSPORT: &[DeckBtn] = &[
    db(DECK_PLAY, (10, 0x02), "PLAY"),
    db(DECK_CUE, (10, 0x04), "CUE"),
    db(DECK_PADMODE1, (13, 0x01), "HOT CUE"),
    db(DECK_PADMODE2, (13, 0x02), "BEAT LOOP"),
    db(DECK_PADMODE3, (13, 0x04), "SLIP LOOP"),
    db(DECK_PADMODE4, (13, 0x08), "BEAT JUMP"),
    db(DECK_PAD1, (14, 0x01), "PAD 1"),
    db(DECK_PAD2, (14, 0x02), "PAD 2"),
    db(DECK_PAD3, (14, 0x04), "PAD 3"),
    db(DECK_PAD4, (14, 0x08), "PAD 4"),
    db(DECK_PAD5, (14, 0x10), "PAD 5"),
    db(DECK_PAD6, (14, 0x20), "PAD 6"),
    db(DECK_PAD7, (14, 0x40), "PAD 7"),
    db(DECK_PAD8, (14, 0x80), "PAD 8"),
];

/// The other deck buttons, at their place on the left deck (the right deck has
/// them at the same place on its own deck, see [`Geom::on_deck`]).
///
/// `bit_l`/`bit_r` = the deck-1 (byte, mask) on the left/right deck; they
/// differ only for DECK SELECT, whose buttons read 3|1 on the left deck and
/// 2|4 on the right. The right deck adds [`ERP_DECK2_STRIDE`].
pub struct DeckBtn {
    pub g: Geom,
    pub bit_l: (usize, u8),
    pub bit_r: (usize, u8),
    pub name: &'static str,
}

const fn db(g: Geom, bit: (usize, u8), name: &'static str) -> DeckBtn {
    DeckBtn {
        g,
        bit_l: bit,
        bit_r: bit,
        name,
    }
}

pub const DECK_ERP: &[DeckBtn] = &[
    db(DECK_IN_CUE, (0x08, 0x40), "LOOP IN/CUE"),
    db(DECK_OUT, (0x08, 0x20), "LOOP OUT"),
    db(DECK_RELOOP, (0x09, 0x08), "RELOOP/EXIT"),
    DeckBtn {
        g: DECK_SELECT_L,
        bit_l: (0x09, 0x01),
        bit_r: (0x08, 0x01),
        name: "DECK SELECT (3 | 2)",
    },
    DeckBtn {
        g: DECK_SELECT_R,
        bit_l: (0x08, 0x01),
        bit_r: (0x09, 0x01),
        name: "DECK SELECT (1 | 4)",
    },
    db(DECK_CALL_PREV, (0x0C, 0x20), "CUE/LOOP CALL <"),
    db(DECK_CALL_NEXT, (0x0C, 0x10), "CUE/LOOP CALL >"),
    db(DECK_DELETE, (0x0C, 0x40), "DELETE"),
    db(DECK_MEMORY, (0x0C, 0x80), "MEMORY"),
    db(DECK_LOOP4, (0x0C, 0x04), "4-BEAT LOOP"),
    db(DECK_LOOP8, (0x0C, 0x08), "8-BEAT LOOP"),
    db(DECK_JUMP_PREV, (0x09, 0x40), "BEAT JUMP <"),
    db(DECK_JUMP_NEXT, (0x09, 0x20), "BEAT JUMP >"),
    db(DECK_SLIP, (0x08, 0x04), "SLIP"),
    db(DECK_SLIP_REV, (0x08, 0x80), "SLIP REVERSE"),
    db(DECK_VINYL_SPEED, (0x0C, 0x01), "VINYL SPEED ADJUST"),
    db(DECK_JOG_MODE, (0x0C, 0x02), "JOG MODE (VINYL)"),
    db(DECK_BEAT_SYNC, (0x08, 0x02), "BEAT SYNC"),
    db(DECK_KEY_SYNC, (0x09, 0x02), "KEY SYNC"),
    db(DECK_MASTER, (0x08, 0x10), "MASTER"),
    db(DECK_TEMPO_RANGE, (0x09, 0x10), "TEMPO RANGE"),
    db(DECK_TRACK_PREV, (0x0B, 0x08), "TRACK SEARCH <"),
    db(DECK_TRACK_NEXT, (0x0B, 0x04), "TRACK SEARCH >"),
    db(DECK_SEARCH_REV, (0x0B, 0x02), "SEARCH <<"),
    db(DECK_SEARCH_FWD, (0x0B, 0x01), "SEARCH >>"),
    db(DECK_TEMPO_RESET, (0x09, 0x04), "TEMPO RESET"),
    db(DECK_MASTER_TEMPO, (0x08, 0x08), "MASTER TEMPO"),
];

/// Erp bits that belong to the whole unit. QUANTIZE is one bit for both decks.
/// USB STOP 1/2 share the event-marker byte 24; pressing one unmounts that
/// stick, as on the deck.
pub const GLOBAL_ERP: &[PanelButton] = &[
    pb(GLOBAL_QUANTIZE, 0x0A, 0x01, "QUANTIZE"),
    pb(USB_STOP1, 24, 0x02, "USB STOP 1"),
    pb(USB_STOP2, 24, 0x04, "USB STOP 2"),
];

/// Panel buttons on the mixer bus. The rects match what `panel::top_center`,
/// `panel::browse_cluster` and `panel::top_sides` draw. MENU is hold-sensitive
/// (a short press opens the screen's menu, 0.3 s or more opens UTILITY), so a
/// press lasts as long as the mouse button is held.
pub const PANEL_BUTTONS: &[PanelButton] = &[
    pb(TOP_SOURCE, 32, 0x80, "SOURCE"),
    pb(TOP_BROWSE, 32, 0x40, "BROWSE"),
    pb(TOP_TAG_LIST, 32, 0x20, "TAG LIST"),
    pb(TOP_PLAYLIST, 32, 0x10, "PLAYLIST"),
    pb(TOP_SEARCH, 32, 0x08, "SEARCH"),
    pb(TOP_MENU, 32, 0x04, "MENU"),
    pb(TOP_BACK, 33, 0x80, "BACK"),
    pb(TOP_TAG_TRACK, 33, 0x40, "TAG TRACK"),
    pb(TOP_TRACK_FILTER, 33, 0x20, "FILTER/EDIT"),
    pb(TOP_SHORTCUT, 33, 0x10, "SHORTCUT"),
    pb(TOP_LOAD1, 33, 0x08, "LOAD 1"),
    pb(TOP_LOAD2, 33, 0x04, "LOAD 2"),
    pb(TOP_LOAD3, 33, 0x02, "LOAD 3"),
    pb(TOP_LOAD4, 33, 0x01, "LOAD 4"),
];

/// Hold every press at least this long. The guest polls the mixer bus at about
/// 16 Hz, and a quick click can be shorter than one poll.
pub const MIN_HOLD: Duration = Duration::from_millis(100);

// ---- Mixer analog controls --------------------------------------------------------

/// One 8-bit mixer control: index into [`MX_BYTE`].
pub type Mx = usize;
pub const MX_CH1_FADER: Mx = 0;
pub const MX_CH2_FADER: Mx = 1;
pub const MX_CH1_TRIM: Mx = 2;
pub const MX_CH2_TRIM: Mx = 3;
pub const MX_CH1_EQ_HI: Mx = 4;
pub const MX_CH1_EQ_MID: Mx = 5;
pub const MX_CH1_EQ_LOW: Mx = 6;
pub const MX_CH2_EQ_HI: Mx = 7;
pub const MX_CH2_EQ_MID: Mx = 8;
pub const MX_CH2_EQ_LOW: Mx = 9;
pub const MX_CH1_COLOR: Mx = 10;
pub const MX_CH2_COLOR: Mx = 11;
pub const MX_SCFX_PARAM: Mx = 12;
pub const MX_CROSSFADER: Mx = 13;
pub const MX_CH3_FADER: Mx = 14;
pub const MX_CH4_FADER: Mx = 15;
pub const MX_MASTER_LEVEL: Mx = 16;
pub const MX_COUNT: usize = 17;
/// Where the deck program reads each control: its high byte, and the byte and
/// shift of its two low bits where known (a 10-bit value; the low bits of four
/// controls share a byte, the first in bits 7-6). Found by setting one byte at a
/// time and reading the deck program's own control object for each named control
/// (`huiid::Mixer::Ch<n>::fader()` and so on) back from its memory.
pub const MX_AT: [(usize, Option<(usize, u8)>); MX_COUNT] = [
    (0x0a, Some((0x0e, 6))), // CH1 fader
    (0x0b, Some((0x0e, 4))), // CH2 fader
    (0x24, Some((0x28, 6))), // CH1 trim
    (0x29, Some((0x2d, 6))), // CH2 trim
    (0x25, Some((0x28, 4))), // CH1 HI
    (0x26, Some((0x28, 2))), // CH1 MID
    (0x27, Some((0x28, 0))), // CH1 LOW
    (0x2a, Some((0x2d, 4))), // CH2 HI
    (0x2b, Some((0x2d, 2))), // CH2 MID
    (0x2c, Some((0x2d, 0))), // CH2 LOW
    (0x3d, Some((0x41, 6))), // CH1 COLOR
    (0x3e, Some((0x41, 4))), // CH2 COLOR
    (0x4e, None),            // Sound Color FX PARAMETER
    (0x0f, Some((0x10, 0))), // crossfader
    (0x0c, Some((0x0e, 2))), // CH3 fader
    (0x0d, Some((0x0e, 0))), // CH4 fader
    (0x38, Some((0x3c, 6))), // master LEVEL
];

/// The mixer frame AZ-EMU sends first: the idle frame with every channel fader,
/// the master LEVEL, the crossfader and every TRIM at the middle of its travel,
/// so a loaded deck is heard as soon as it plays. (The idle frame was captured
/// with the faders and master down.) The controls move freely from there.
pub fn initial_mixer_frame() -> Frame {
    const MID: u16 = 512;
    let mut f = SPI3_IDLE;
    for mx in [
        MX_CH1_FADER,
        MX_CH2_FADER,
        MX_CH3_FADER,
        MX_CH4_FADER,
        MX_MASTER_LEVEL,
        MX_CROSSFADER,
        MX_CH1_TRIM,
        MX_CH2_TRIM,
    ] {
        if let (hi, Some((lo, shift))) = MX_AT[mx] {
            set10(&mut f, hi, lo, shift, MID);
        }
    }
    // Channel 3 and 4's TRIM are in MX10.
    for &(_, hi, lo, shift, name) in MX10 {
        if name.ends_with("TRIM") {
            set10(&mut f, hi, lo, shift, MID);
        }
    }
    f
}

/// A control's position (0..1) in a mixer frame.
pub fn get_mx(f: &Frame, mx: Mx) -> f32 {
    let v = match MX_AT[mx] {
        (hi, Some((lo, shift))) => u16::from(f[hi]) << 2 | u16::from((f[lo] >> shift) & 3),
        (hi, None) => u16::from(f[hi]) << 2,
    };
    f32::from(v) / 1023.0
}

/// Put a control's position (0..1) into the mixer frame.
pub fn set_mx(f: &mut Frame, mx: Mx, t: f32) {
    let v = (t.clamp(0.0, 1.0) * 1023.0).round() as u16;
    match MX_AT[mx] {
        (hi, Some((lo, shift))) => set10(f, hi, lo, shift, v),
        (hi, None) => f[hi] = (v >> 2) as u8,
    }
}

/// 10-bit mixer knobs drawn by the app over the panel: (where, high byte,
/// low-bits byte, low-bits shift, name). Channel 3 and 4's are confirmed the
/// same way as `MX_AT`; the rest were measured on a deck, one knob swept at a
/// time. The four knobs sharing a low-bits byte take shifts 6, 4, 2 and 0 in
/// the order of their high bytes, and each sweep moved only its own two bits.
/// MIC 1 and MIC 2 LEVEL are not here: they are analog gain and on no bus.
pub const MX10: &[(Geom, usize, usize, u8, &str)] = &[
    (MIXER_TRIM_3, 0x2e, 0x32, 6, "CH3 TRIM"),
    (MIXER_HI_3, 0x2f, 0x32, 4, "CH3 HI"),
    (MIXER_MID_3, 0x30, 0x32, 2, "CH3 MID"),
    (MIXER_LOW_3, 0x31, 0x32, 0, "CH3 LOW"),
    (MIXER_TRIM_4, 0x33, 0x37, 6, "CH4 TRIM"),
    (MIXER_HI_4, 0x34, 0x37, 4, "CH4 HI"),
    (MIXER_MID_4, 0x35, 0x37, 2, "CH4 MID"),
    (MIXER_LOW_4, 0x36, 0x37, 0, "CH4 LOW"),
    (MASTER_HI, 0x39, 0x3c, 4, "MASTER HI"),
    (MASTER_MID, 0x3a, 0x3c, 2, "MASTER MID"),
    (MASTER_LOW, 0x3b, 0x3c, 0, "MASTER LOW"),
    (MIXER_COLOR_3, 0x3f, 0x41, 2, "CH3 COLOR"),
    (MIXER_COLOR_4, 0x40, 0x41, 0, "CH4 COLOR"),
    (MIC_HI1, 0x42, 0x46, 6, "MIC 1 HI"),
    (MIC_MID1, 0x43, 0x46, 4, "MIC 1 MID"),
    (MIC_LOW1, 0x44, 0x46, 2, "MIC 1 LOW"),
    (FX_LEVEL_DEPTH, 0x45, 0x46, 0, "BEAT FX LEVEL/DEPTH"),
    (MIC_HI2, 0x47, 0x4b, 6, "MIC 2 HI"),
    (MIC_MID2, 0x48, 0x4b, 4, "MIC 2 MID"),
    (MIC_LOW2, 0x49, 0x4b, 2, "MIC 2 LOW"),
    (MASTER_BOOTH, 0x4a, 0x4b, 0, "BOOTH LEVEL"),
    (MIXER_HP_MIX, 0x4c, 0x50, 6, "HEADPHONES MIX"),
    (MIXER_HP_LEVEL, 0x4d, 0x50, 4, "HEADPHONES LEVEL"),
];

/// Mixer-bus buttons, measured on a deck: the bit is set while the button is
/// down. The Sound Color FX and Beat FX channel buttons are listed in reading
/// order (left to right, then the next row).
pub const MIXER_BUTTONS: &[PanelButton] = &[
    pb(MIXER_CUE_1, 0x14, 0x08, "CH1 CUE"),
    pb(MIXER_CUE_2, 0x14, 0x04, "CH2 CUE"),
    pb(MIXER_CUE_3, 0x14, 0x02, "CH3 CUE"),
    pb(MIXER_CUE_4, 0x14, 0x01, "CH4 CUE"),
    pb(FX_BEAT_NEXT, 0x14, 0x10, "BEAT >"),
    pb(FX_BEAT_PREV, 0x14, 0x20, "BEAT <"),
    pb(FX_TAP, 0x14, 0x80, "TAP"),
    pb(MIXER_SCFX_SPACE, 0x15, 0x02, "SPACE"),
    pb(MIXER_SCFX_DUBECHO, 0x15, 0x10, "DUB ECHO"),
    pb(MIXER_SCFX_CRUSH, 0x15, 0x01, "CRUSH"),
    pb(MIXER_SCFX_SWEEP, 0x15, 0x08, "SWEEP"),
    pb(MIXER_SCFX_NOISE, 0x15, 0x20, "NOISE"),
    pb(MIXER_SCFX_FILTER, 0x15, 0x04, "FILTER"),
    pb(MASTER_CUE, 0x15, 0x40, "MASTER CUE"),
    pb(MIXER_LINK_CUE, 0x15, 0x80, "LINK CUE"),
    pb(FX_FREQ_LOW, 0x16, 0x02, "FX FREQUENCY LOW"),
    pb(FX_FREQ_MID, 0x16, 0x04, "FX FREQUENCY MID"),
    pb(FX_FREQ_HI, 0x16, 0x08, "FX FREQUENCY HI"),
    pb(FX_CH_3, 0x17, 0x20, "BEAT FX CH 3"),
    pb(FX_CH_1, 0x17, 0x80, "BEAT FX CH 1"),
    pb(FX_CH_2, 0x17, 0x40, "BEAT FX CH 2"),
    pb(FX_CH_4, 0x17, 0x10, "BEAT FX CH 4"),
    pb(FX_CH_MIC, 0x17, 0x01, "BEAT FX MIC"),
    pb(FX_CH_CFA, 0x17, 0x08, "BEAT FX CF.A"),
    pb(FX_CH_CFB, 0x17, 0x04, "BEAT FX CF.B"),
    pb(FX_CH_MST, 0x17, 0x02, "BEAT FX MASTER"),
    pb(FX_QUANTIZE, 0x18, 0x04, "FX QUANTIZE"),
    pb(FX_AUTO_TAP, 0x18, 0x08, "AUTO/TAP"),
    pb(MIC_TALK_OVER, 0x18, 0x20, "TALK OVER"),
    pb(MIC_ON2, 0x18, 0x40, "MIC 2 ON"),
    pb(MIC_ON1, 0x18, 0x80, "MIC 1 ON"),
];

/// Beat FX TIME, an endless encoder: a signed 16-bit counter, low byte first,
/// one count per detent.
pub const FX_TIME_BYTES: (usize, usize) = (0x1a, 0x1b);
pub const FX_TIME_KNOB: Geom = FX_TIME;

/// A 10-bit mixer value from its high byte and the two low bits at `shift`.
pub fn get10(f: &Frame, hi: usize, lo: usize, shift: u8) -> u16 {
    u16::from(f[hi]) << 2 | u16::from((f[lo] >> shift) & 3)
}

/// Beat FX: SELECT = mixer 0x16 high nibble (Gray code), ON/OFF = bit 0.
/// Measured on the deck; the emulator's deck program does not change its FX
/// panel for either, so they are unconfirmed there.
pub const MX_FXSEL_BYTE: usize = 0x16;
pub const FX_SELECT_KNOB: Geom = FX_SELECT;
pub const FX_ON_BTN: Geom = FX_ON;

// ---- Encoders ---------------------------------------------------------------------

/// Tempo fader position for a drawn fader `pos` (0 = bottom, 1 = top). The
/// fader is a 16-bit little-endian value in deck bytes 16-17, about 11..1023
/// with the center at 517; pulling the fader toward you (down) is +, as on the
/// hardware.
pub fn tempo_raw(pos: f32) -> u16 {
    (517.0 + (0.5 - pos.clamp(0.0, 1.0)) * 2.0 * 506.0).round() as u16
}

/// Write a 10-bit mixer value: the top 8 bits at `hi`, the low 2 bits into
/// `lo` at bit `shift`.
pub fn set10(f: &mut Frame, hi: usize, lo: usize, shift: u8, v: u16) {
    let v = v.min(1023);
    f[hi] = (v >> 2) as u8;
    f[lo] = (f[lo] & !(3 << shift)) | (((v & 3) as u8) << shift);
}

/// X-PAD zone bitmap for a 7-bit position: one or two bits walk across the
/// strip. Never 0 while touched; 0 means the finger is up.
pub fn xpad_zone_bits(pos: u8) -> u8 {
    const BITS: [u8; 14] = [
        0x01, 0x03, 0x02, 0x06, 0x04, 0x0c, 0x08, 0x18, 0x10, 0x30, 0x20, 0x60, 0x40, 0xc0,
    ];
    BITS[((pos & 0x7f) as usize * BITS.len() / 128).min(BITS.len() - 1)]
}

/// 4-bit Gray code (the Beat FX SELECT knob reports its position this way).
pub fn gray4(p: i32) -> u8 {
    let p = p.rem_euclid(16) as u8;
    p ^ (p >> 1)
}

/// Jog fields of a deck frame, as a real deck sends them: byte 9 = touched and
/// turning forward (0x0f) or back (0x0b), 0 when released; absolute position = u16 LE
/// 10-11; bytes 12-13 = u16 LE PERIOD, not a velocity: 0xFFFF while the platter is
/// still, small while it turns. A touched platter released with 0 there reads as an
/// infinitely fast backward spin: the track runs back to its start and PLAY stops
/// working until CUE.
pub const JOG_TOUCH_FWD: u8 = 0x0f;
pub const JOG_TOUCH_BACK: u8 = 0x0b;
pub const JOG_STOPPED: u16 = 0xffff;
/// The period while turning: the real deck's median while turned by hand; no speed curve.
pub const JOG_TURNING: u16 = 450;

pub fn jog_into(f: &mut Frame, touch: u8, pos: u16, period: u16) {
    f[9] = touch;
    f[10..12].copy_from_slice(&pos.to_le_bytes());
    f[12..14].copy_from_slice(&period.to_le_bytes());
}

/// Tempo fader of a deck frame (see [`tempo_raw`]).
pub fn tempo_into(f: &mut Frame, raw: u16) {
    f[16..18].copy_from_slice(&raw.to_le_bytes());
}

// ---- Pad LEDs -----------------------------------------------------------------------

/// One write of the deck program to a panel bus, as the guest forwards it: the
/// longest is a deck bus's 152 bytes; the erp and mixer buses write 128 and
/// arrive zero-padded.
///
/// The erp pad LED frame: 0x00 a signature, 0x09..0x0c packed state codes,
/// 0x0e + 4j the (R, G, B, 0) of pad-mode button j, 0x1e + 4k the (R, G, B, 0)
/// of pad k, deck 2 the same at +0x38, and a CRC-16/X-25 at 0x78. The RGB bytes
/// are linear light, not sRGB: the deck program dims a pad by scaling all three
/// bytes by one factor (98 ff 9a lit, 07 0c 07 dimmed), so they are converted to
/// sRGB for the screen ([`led_color`]).
pub const LED_FRAME_SIZE: usize = 152;
const LED_PAD_OFF_DECK1: usize = 0x1e;
const LED_PADMODE_OFF_DECK1: usize = 0x0e;
const LED_DECK_STRIDE: usize = 0x38;
const LED_PAD_STRIDE: usize = 4;

/// What the guest sends per LED write: `[bus] + frame + the write's length (LE
/// u32)`. The guest captures writes on every bus into one buffer, so the bus tag
/// is what tells an erp pad-LED frame from a deck or mixer write.
pub const LED_WIRE_SIZE: usize = 1 + LED_FRAME_SIZE + 4;
pub type LedWire = [u8; LED_WIRE_SIZE];

/// Deck button LEDs in the erp frame: a 2-bit level (0 off, 1 dim, 2 on; the
/// deck program blinks a light by alternating levels) at (byte, shift), on the
/// left deck; the right deck's are [`ERP_LED_DECK_STRIDE`] bytes on. Measured
/// on the emulator by toggling each button, and PLAY and CUE by playing and
/// pausing, with every other field unchanged. IN/CUE and OUT are lit while no
/// loop plays and blink while a loop is being set or plays; RELOOP/EXIT is lit
/// while the deck holds a loop.
pub const DECK_LEDS: &[(Geom, usize, u8)] = &[
    (DECK_PLAY, 0x0b, 0),
    (DECK_CUE, 0x0b, 2),
    (DECK_IN_CUE, 0x0a, 6),
    (DECK_OUT, 0x0a, 4),
    (DECK_RELOOP, 0x09, 0),
    (DECK_SLIP, 0x08, 0),
    (DECK_MASTER_TEMPO, 0x08, 2),
    (DECK_BEAT_SYNC, 0x09, 6),
    // Level 2 on the tempo master deck, 1 on the other.
    (DECK_MASTER, 0x08, 4),
    // Level 1 while it can be pressed, 2 on, 0 on the master deck (nothing to
    // follow).
    (DECK_KEY_SYNC, 0x09, 2),
    (DECK_JOG_MODE, 0x0c, 4),
    (DECK_VINYL_SPEED, 0x0c, 6),
];
/// The right deck's LEDs follow the left deck's this many bytes on.
pub const ERP_LED_DECK_STRIDE: usize = 0x38;
/// QUANTIZE: one LED for the unit.
pub const QUANTIZE_LED: (usize, u8) = (0x0a, 2);

/// A 2-bit LED level from an erp frame.
pub fn led_level(erp: &[u8], byte: usize, shift: u8) -> u8 {
    erp.get(byte).map_or(0, |b| (b >> shift) & 3)
}

/// Both decks' pad colors from one LED write, or None if it came from another
/// bus. A pad that is off is black.
pub fn parse_pad_leds(wire: &LedWire) -> Option<[[Color32; 8]; 2]> {
    if wire[0] != BUS_ERP {
        return None;
    }
    let frame = &wire[1..1 + LED_FRAME_SIZE];
    let mut out = [[Color32::BLACK; 8]; 2];
    for (deck, pads) in out.iter_mut().enumerate() {
        for (k, pad) in pads.iter_mut().enumerate() {
            let o = LED_PAD_OFF_DECK1 + deck * LED_DECK_STRIDE + k * LED_PAD_STRIDE;
            *pad = led_color([frame[o], frame[o + 1], frame[o + 2]]);
        }
    }
    Some(out)
}

/// Pad-mode button `j`'s LED color on deck `d` (0 left, 1 right), from an erp
/// LED frame (without the bus tag); None while the deck program has it off.
pub fn padmode_led(erp: &[u8], d: usize, j: usize) -> Option<Color32> {
    let o = LED_PADMODE_OFF_DECK1 + d * LED_DECK_STRIDE + j * LED_PAD_STRIDE;
    let rgb = erp.get(o..o + 3)?;
    (rgb != [0, 0, 0]).then(|| led_color_with([rgb[0], rgb[1], rgb[2]], WHITE_PADMODE))
}

/// What the deck program sends for white, by kind of LED: the pad-mode buttons
/// (HOT CUE's white) and the pads (SLIP LOOP's pad 7). The red, green and blue
/// parts are driven unequally to make white, and differently for the two kinds,
/// so each channel is measured against its kind's white.
pub const WHITE_PADMODE: [u8; 3] = [0x98, 0xff, 0x9a];
pub const WHITE_PAD: [u8; 3] = [0xb3, 0xeb, 0x5a];

/// An LED's linear-light RGB bytes as an sRGB color for the screen, with
/// `white` (one of [`WHITE_PADMODE`], [`WHITE_PAD`]) shown as white. Shown as
/// sent, a dim pad would look nearly black, every mixed color too saturated,
/// and the deck's white green.
pub fn led_color_with(rgb: [u8; 3], white: [u8; 3]) -> Color32 {
    let ch = |v: u8, w: u8| {
        let lin = (f32::from(v) / f32::from(w.max(1))).min(1.0);
        egui::ecolor::gamma_u8_from_linear_f32(lin)
    };
    Color32::from_rgb(
        ch(rgb[0], white[0]),
        ch(rgb[1], white[1]),
        ch(rgb[2], white[2]),
    )
}

/// A pad's LED color for the screen (see [`led_color_with`]).
pub fn led_color(rgb: [u8; 3]) -> Color32 {
    led_color_with(rgb, WHITE_PAD)
}

// ---- Audio levels ----------------------------------------------------------------

/// Channels in the deck program's audio stream: a stereo pair per deck (deck n
/// on channels 2n-2 and 2n-1), each the deck's output before the mixer, then
/// two that stayed silent in every test.
pub const LEVEL_CHANNELS: usize = 10;

/// One measurement of every channel, over about 23 ms: the largest sample and
/// the RMS, as fractions of full scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub peak: [f32; LEVEL_CHANNELS],
    pub rms: [f32; LEVEL_CHANNELS],
}

/// The levels in a [`BUS_LEVELS`] wire: a sequence number, then each channel's
/// peak, then each channel's RMS, as little-endian floats.
pub fn parse_levels(wire: &LedWire) -> Option<Levels> {
    if wire[0] != BUS_LEVELS {
        return None;
    }
    let f = |k: usize| {
        let o = 1 + 4 + k * 4;
        f32::from_le_bytes([wire[o], wire[o + 1], wire[o + 2], wire[o + 3]])
    };
    Some(Levels {
        peak: std::array::from_fn(f),
        rms: std::array::from_fn(|ch| f(LEVEL_CHANNELS + ch)),
    })
}

// ---- The jog display ------------------------------------------------------------------

/// Messages to a deck's jog display ride in the deck frames, one at a time in a
/// fixed slot: a present flag, a sequence number (a repeated frame repeats it),
/// the display (0x10 left, 0x20 right), the message id, then a piece number
/// (from 1), the number of pieces and the piece's data. When a deck loads a
/// track the deck program first sends [`MSG_LOAD`], then, if the track has
/// artwork, the image in a few hundred [`MSG_ART`] pieces: the first piece
/// starts with the image's length (3 bytes, little-endian) and the image is a
/// JPEG.
const MSG_PRESENT: usize = 0x4c;
const MSG_SEQ: usize = 0x4e;
const MSG_DISPLAY: usize = 0x50;
const MSG_ID: usize = 0x51;
const MSG_INDEX: usize = 0x52;
const MSG_TOTAL: usize = 0x54;
const MSG_DATA: usize = 0x56;
const MSG_DATA_LEN: usize = 58;
const DISPLAYS: [u8; 2] = [0x10, 0x20];
const MSG_LOAD: u8 = 0x2d;
const MSG_OVERVIEW: u8 = 0x2c;
const MSG_ART: u8 = 0x33;

/// One column of a track's overview waveform, as the jog display gets it: its
/// color, its full height and the height of its brighter lower part (0..=40 or
/// so: 39 was the largest seen).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveColumn {
    pub color: Color32,
    pub height: u8,
    pub inner: u8,
}

/// The overview waveform message's columns: after one byte (0x90 on every
/// track seen), 7 bytes each, `[RGB565 LE, 0, 0, height, inner height, 0]`,
/// up to the first empty one. Measured on the emulator: the columns of two
/// tracks drew the same shapes as the deck program's own overview of each, and
/// not each other's.
pub fn parse_overview(data: &[u8]) -> Vec<WaveColumn> {
    data.get(1..)
        .unwrap_or_default()
        .chunks(7)
        .take_while(|c| c.len() == 7 && c.iter().any(|&b| b != 0))
        .map(|c| {
            let v = u16::from_le_bytes([c[0], c[1]]);
            let (r, g, b) = (v >> 11, (v >> 5) & 0x3f, v & 0x1f);
            WaveColumn {
                color: Color32::from_rgb(
                    (r * 255 / 31) as u8,
                    (g * 255 / 63) as u8,
                    (b * 255 / 31) as u8,
                ),
                height: c[4],
                inner: c[5],
            }
        })
        .collect()
}

/// What a deck frame told its jog display.
#[derive(Debug, PartialEq)]
pub enum JogEvent {
    /// A track is being loaded: whatever the display showed is gone. The deck
    /// program sends a few of these per load; the last one carries the track's
    /// length (ms), the others 0.
    Load(u32),
    /// The new track's overview waveform.
    Overview(Vec<WaveColumn>),
    /// The new track's artwork, a whole JPEG.
    Art(Vec<u8>),
}

/// A message sent in numbered pieces (from 1), put back together. A missing
/// piece drops that copy.
#[derive(Default)]
struct Pieces {
    data: Vec<u8>,
    next: u16,
}

impl Pieces {
    /// Take a piece; the whole message once its last piece is in.
    fn add(&mut self, index: u16, total: u16, data: &[u8]) -> Option<Vec<u8>> {
        if index == 1 {
            self.data.clear();
            self.next = 1;
        }
        if index == 0 || index != self.next {
            self.next = 0;
            return None;
        }
        self.data.extend_from_slice(data);
        self.next += 1;
        if index != total {
            return None;
        }
        self.next = 0;
        Some(std::mem::take(&mut self.data))
    }
}

/// Follows one deck bus's jog display messages.
#[derive(Default)]
pub struct JogDisplay {
    seq: Option<u16>,
    art: Pieces,
    overview: Pieces,
}

impl JogDisplay {
    /// Take a deck frame. A missing piece drops that image or waveform; the next
    /// load sends a new one.
    pub fn feed(&mut self, frame: &[u8]) -> Option<JogEvent> {
        if frame.len() < MSG_DATA + MSG_DATA_LEN
            || frame[MSG_PRESENT] != 1
            || !DISPLAYS.contains(&frame[MSG_DISPLAY])
        {
            return None;
        }
        let le = |o: usize| u16::from_le_bytes([frame[o], frame[o + 1]]);
        let seq = le(MSG_SEQ);
        if self.seq.replace(seq) == Some(seq) {
            return None;
        }
        let data = &frame[MSG_DATA..MSG_DATA + MSG_DATA_LEN];
        let (index, total) = (le(MSG_INDEX), le(MSG_TOTAL));
        match frame[MSG_ID] {
            // A 24-bit length after one byte. Pieces under way are kept: the
            // deck program can send an overview's first piece before the last
            // of these.
            MSG_LOAD => Some(JogEvent::Load(
                u32::from(data[1]) | u32::from(data[2]) << 8 | u32::from(data[3]) << 16,
            )),
            MSG_OVERVIEW => self
                .overview
                .add(index, total, data)
                .map(|d| JogEvent::Overview(parse_overview(&d))),
            MSG_ART => {
                let art = self.art.add(index, total, data)?;
                // The image's length (3 bytes), then the JPEG.
                let len = usize::from(*art.first()?)
                    | usize::from(*art.get(1)?) << 8
                    | usize::from(*art.get(2)?) << 16;
                art.get(3..3 + len).map(|jpeg| JogEvent::Art(jpeg.to_vec()))
            }
            _ => None,
        }
    }
}

/// The deck frames' status records: byte 8 names the deck a frame describes,
/// 0x31 deck 1 on the left bus and 0x41 deck 2 on the right (each bus's other
/// record, 0x11 and 0x21, read "no track" in every capture: decks 3 and 4).
const RECORD_ID: usize = 8;
const RECORD_DECK_1: u8 = 0x31;
const RECORD_DECK_2: u8 = 0x41;
/// Where a status record holds the elapsed time: minutes, seconds, then
/// milliseconds as u16 LE; 0xbf in the minutes when no track is loaded.
const RECORD_ELAPSED: usize = 0x24;

/// The elapsed time (ms) in a deck frame from bus side `d` (0 left, 1 right),
/// if it is that side's deck 1 or 2 record and a track is loaded.
pub fn elapsed_ms(frame: &[u8], d: usize) -> Option<u32> {
    let id = if d == 0 { RECORD_DECK_1 } else { RECORD_DECK_2 };
    if frame.get(RECORD_ID) != Some(&id) {
        return None;
    }
    let t = frame.get(RECORD_ELAPSED..RECORD_ELAPSED + 4)?;
    if t[0] == 0xbf {
        return None;
    }
    let ms = u32::from(u16::from_le_bytes([t[2], t[3]]));
    Some((u32::from(t[0]) * 60 + u32::from(t[1])) * 1000 + ms)
}

// ---- The socket ---------------------------------------------------------------------

/// A whole bus frame to send. Each bus keeps one overlay that every frame
/// replaces, so the app always sends the full state of a bus (the jog and the
/// tempo fader together, every mixer level with each button).
pub struct CtrlMsg {
    pub bus: u8,
    pub frame: Frame,
}

/// The 153 bytes the socket expects: the bus id, then the frame.
pub fn wire_frame(msg: &CtrlMsg) -> [u8; 153] {
    let mut f = [0u8; 153];
    f[0] = msg.bus;
    f[1..].copy_from_slice(&msg.frame);
    f
}

/// Keep a connection to `ctrl.sock`, send every [`CtrlMsg`], and pass LED
/// frames back to the UI. Reconnects when the guest restarts; returns when the
/// UI is gone. `AZ_CTRL_LOG=1` logs every frame sent (hex) to a scratch file.
/// The latest frame sent on each bus. The app sends a bus's whole state with
/// every frame, so these are the panel as it stands; they are sent again
/// whenever the socket (re)connects, so the guest starts from the app's state
/// (its initial mixer positions, for one) rather than the idle frames.
#[derive(Default)]
pub struct LatestFrames([Option<Frame>; 4]);

impl LatestFrames {
    pub fn note(&mut self, msg: &CtrlMsg) {
        if let Some(slot) = self.0.get_mut(usize::from(msg.bus)) {
            *slot = Some(msg.frame);
        }
    }

    /// Every bus's latest frame, in bus order.
    pub fn messages(&self) -> Vec<CtrlMsg> {
        (0u8..)
            .zip(&self.0)
            .filter_map(|(bus, f)| f.map(|frame| CtrlMsg { bus, frame }))
            .collect()
    }
}

/// The bytes of a mixer frame that hold analog levels (every control in [`MX_AT`]
/// and [`MX10`], high and low bits): what az-drive must keep to stay in step.
pub fn mixer_levels(frame: &Frame) -> Vec<(usize, u8)> {
    let mut at: Vec<usize> = Vec::new();
    for &(hi, lo) in &MX_AT {
        at.push(hi);
        if let Some((lo, _)) = lo {
            at.push(lo);
        }
    }
    for &(_, hi, lo, _, _) in MX10 {
        at.push(hi);
        at.push(lo);
    }
    at.sort_unstable();
    at.dedup();
    at.into_iter()
        .filter(|&i| i < frame.len())
        .map(|i| (i, frame[i]))
        .collect()
}

pub fn ctrl_thread(rx: Receiver<CtrlMsg>, led_tx: Sender<LedWire>) {
    let log = std::env::var_os("AZ_CTRL_LOG").is_some();
    let mut latest = LatestFrames::default();
    let send = |stream: &mut UnixStream, msg: &CtrlMsg| {
        let frame = wire_frame(msg);
        if log {
            append_hex(&scratch("az-ctrl-sent.log"), "", &frame);
        }
        stream.write_all(&frame)
    };
    loop {
        let mut stream = match UnixStream::connect(sock_dir().join("ctrl.sock")) {
            Ok(s) => s,
            Err(_) => {
                // Nothing to send to while disconnected: keep the latest state.
                while let Ok(msg) = rx.try_recv() {
                    latest.note(&msg);
                }
                thread::sleep(Duration::from_millis(700));
                continue;
            }
        };
        // The socket is full duplex: read LED frames on a cloned handle so a
        // blocked write never stalls them, and the other way round.
        if let Ok(read_half) = stream.try_clone() {
            let led_tx = led_tx.clone();
            thread::spawn(move || read_led_frames(read_half, led_tx));
        }
        let replay = latest.messages();
        if replay.iter().any(|m| send(&mut stream, m).is_err()) {
            continue; // guest gone: reconnect
        }
        // After the boot scripts cleared it: az-drive starts from the app's mixer.
        if let Some(m) = replay.iter().find(|m| m.bus == BUS_MIXER) {
            mixer_state_store(&mixer_levels(&m.frame));
        }
        loop {
            let Ok(msg) = rx.recv() else { return };
            latest.note(&msg);
            if send(&mut stream, &msg).is_err() {
                break; // guest gone: reconnect
            }
            if msg.bus == BUS_MIXER {
                mixer_state_store(&mixer_levels(&msg.frame));
            }
        }
    }
}

/// Forward LED frames until the connection or the UI goes away.
fn read_led_frames(mut stream: UnixStream, led_tx: Sender<LedWire>) {
    loop {
        let mut wire = [0u8; LED_WIRE_SIZE];
        if stream.read_exact(&mut wire).is_err() || led_tx.send(wire).is_err() {
            return;
        }
    }
}

/// Append one line of hex to a debug log; errors are ignored (it is a log).
pub fn append_hex(path: &std::path::Path, prefix: &str, bytes: &[u8]) {
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = writeln!(f, "{prefix}{}", hex.join(" "));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with one button bit set, as the app composes it.
    fn with_bit(idle: Frame, byte: usize, mask: u8) -> Frame {
        let mut f = idle;
        f[byte] |= mask;
        f
    }

    #[test]
    fn deck_frame_jog_and_tempo_together() {
        let mut d = SPI5_IDLE;
        jog_into(&mut d, JOG_TOUCH_FWD, 6500, 400);
        tempo_into(&mut d, tempo_raw(1.0));
        let f = wire_frame(&CtrlMsg {
            bus: BUS_DECK_L,
            frame: d,
        });
        assert_eq!(f[0], BUS_DECK_L);
        assert_eq!(&f[1 + 9..1 + 14], &[0x0f, 0x64, 0x19, 0x90, 0x01]);
        // The tempo survives a later jog update (one frame carries both).
        jog_into(&mut d, 0, 6500, JOG_STOPPED);
        assert_eq!(u16::from_le_bytes([d[16], d[17]]), tempo_raw(1.0));
        assert_eq!(&d[0..6], &SPI5_IDLE[0..6]);
        // A release says "stopped" (0xFFFF), never 0, which is an infinitely fast spin.
        assert_eq!((d[9], d[12], d[13]), (0, 0xff, 0xff));
        // Backward is the touch byte, as on a deck; the period stays positive.
        jog_into(&mut d, JOG_TOUCH_BACK, 6500, JOG_TURNING);
        assert_eq!((d[9], u16::from_le_bytes([d[12], d[13]])), (0x0b, 450));
    }

    #[test]
    fn tempo_is_u16_le_centered_at_517() {
        assert_eq!(tempo_raw(0.5), 517);
        assert!(tempo_raw(0.0) > 1000); // pulled down (toward you) = +
        assert!(tempo_raw(1.0) < 20); // pushed up = -
        let mut d = SPI2_IDLE;
        tempo_into(&mut d, 0x0246);
        assert_eq!((d[16], d[17]), (0x46, 0x02));
    }

    #[test]
    fn deck_buttons_match_the_deck_field_table() {
        let want: &[(&str, (usize, u8))] = &[
            ("LOOP IN/CUE", (0x08, 0x40)),
            ("LOOP OUT", (0x08, 0x20)),
            ("RELOOP/EXIT", (0x09, 0x08)),
            ("CUE/LOOP CALL <", (0x0C, 0x20)),
            ("CUE/LOOP CALL >", (0x0C, 0x10)),
            ("DELETE", (0x0C, 0x40)),
            ("MEMORY", (0x0C, 0x80)),
            ("4-BEAT LOOP", (0x0C, 0x04)),
            ("8-BEAT LOOP", (0x0C, 0x08)),
            ("BEAT JUMP <", (0x09, 0x40)),
            ("BEAT JUMP >", (0x09, 0x20)),
            ("SLIP", (0x08, 0x04)),
            ("SLIP REVERSE", (0x08, 0x80)),
            ("VINYL SPEED ADJUST", (0x0C, 0x01)),
            ("JOG MODE (VINYL)", (0x0C, 0x02)),
            ("BEAT SYNC", (0x08, 0x02)),
            ("KEY SYNC", (0x09, 0x02)),
            ("TRACK SEARCH <", (0x0B, 0x08)),
            ("TRACK SEARCH >", (0x0B, 0x04)),
            ("SEARCH <<", (0x0B, 0x02)),
            ("SEARCH >>", (0x0B, 0x01)),
            ("TEMPO RESET", (0x09, 0x04)),
            ("MASTER TEMPO", (0x08, 0x08)),
            ("MASTER", (0x08, 0x10)),
            ("TEMPO RANGE", (0x09, 0x10)),
        ];
        for (name, bit) in want {
            let b = DECK_ERP
                .iter()
                .find(|b| b.name == *name)
                .unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!((b.bit_l, b.bit_r), (*bit, *bit), "{name}");
        }
        // DECK SELECT: left slots 3|1 = secondary|primary, right 2|4 = primary|secondary.
        let ds: Vec<_> = DECK_ERP
            .iter()
            .filter(|b| b.name.starts_with("DECK SELECT"))
            .collect();
        assert_eq!((ds[0].bit_l, ds[0].bit_r), ((0x09, 0x01), (0x08, 0x01)));
        assert_eq!((ds[1].bit_l, ds[1].bit_r), ((0x08, 0x01), (0x09, 0x01)));
        // Every bit is distinct on each deck and clear of PLAY/CUE, SHIFT and QUANTIZE.
        for side in 0..2 {
            let mut seen: Vec<(usize, u8)> =
                vec![(0x0A, 0x02), (0x0A, 0x04), ERP_SHIFT, (0x0A, 0x01)];
            for b in DECK_ERP {
                let bit = if side == 0 { b.bit_l } else { b.bit_r };
                assert!(
                    (0x08..=0x0C).contains(&bit.0),
                    "{} outside the deck section",
                    b.name
                );
                assert!(!seen.contains(&bit), "{} collides", b.name);
                seen.push(bit);
            }
        }
    }

    #[test]
    fn ten_bit_packing_and_gray_code() {
        let mut f = SPI3_IDLE;
        set10(&mut f, 0x2e, 0x32, 6, 1023); // CH3 TRIM max
        assert_eq!(f[0x2e], 0xff);
        assert_eq!(f[0x32] >> 6, 3);
        set10(&mut f, 0x31, 0x32, 0, 513); // CH3 LOW shares the low-bits byte
        assert_eq!((f[0x31], f[0x32] & 3), (128, 1));
        assert_eq!(f[0x32] >> 6, 3, "TRIM's low bits survive a LOW write");
        for p in 0..16 {
            assert_eq!((gray4(p) ^ gray4(p + 1)).count_ones(), 1);
        }
        assert_eq!(gray4(1) << 4, SPI3_IDLE[MX_FXSEL_BYTE]); // idle = position 1
    }

    #[test]
    fn mixer_bytes_are_distinct_and_in_range() {
        let hi: Vec<usize> = MX_AT.iter().map(|&(h, _)| h).collect();
        let mut sorted = hi.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), hi.len(), "MX_AT high bytes must be distinct");
        for &(h, lo) in &MX_AT {
            assert!(h < 0x60, "inside the CRC-covered part of the frame");
            if let Some((l, shift)) = lo {
                assert!(l < 0x60 && shift % 2 == 0 && shift <= 6 && !hi.contains(&l));
            }
        }
        assert!(!hi.contains(&KNOB_BYTE) && !hi.contains(&LATCH_BYTE));
        assert_eq!(
            SPI3_IDLE[KNOB_BYTE], 128,
            "the knob counter starts mid-range"
        );
    }

    #[test]
    fn panel_and_erp_bits_never_collide() {
        assert_eq!(&SPI3_IDLE[0..6], &[3, 1, 2, 1, 1, 1]);
        assert_eq!(SPI3_IDLE[LATCH_BYTE], 0x08);
        for row in [32usize, 33] {
            let mut seen = 0u8;
            for b in PANEL_BUTTONS.iter().filter(|b| b.byte == row) {
                assert_eq!(seen & b.mask, 0, "duplicate bit in byte {row}: {}", b.name);
                seen |= b.mask;
            }
        }
        let mut seen = std::collections::HashSet::new();
        for b in DECK_TRANSPORT {
            assert_eq!(b.bit_l, b.bit_r, "{}", b.name);
            assert!(seen.insert(b.bit_l), "duplicate transport bit: {}", b.name);
        }
        // Each deck button owns its bit on each deck (DECK SELECT swaps two).
        for side in 0..2 {
            let mut seen = std::collections::HashSet::new();
            for b in DECK_ERP {
                let bit = if side == 0 { b.bit_l } else { b.bit_r };
                assert!(seen.insert(bit), "duplicate deck bit {bit:?}: {}", b.name);
            }
        }
        // Deck 2 is deck 1 plus the stride; the event marker rides every frame.
        let play2 = with_bit(ERP_IDLE, 10 + ERP_DECK2_STRIDE, 0x02);
        assert_eq!((play2[18], play2[10], play2[24]), (0x02, 0x00, 0x01));
    }

    #[test]
    fn led_frames_from_other_buses_are_ignored() {
        let mut wire = [0u8; LED_WIRE_SIZE];
        wire[1 + 0x1e..1 + 0x21].copy_from_slice(&[10, 20, 30]); // deck 1 pad 1
        wire[1 + 0x56 + 4 * 7..1 + 0x56 + 4 * 7 + 3].copy_from_slice(&[1, 2, 3]); // deck 2 pad 8
        let pads = parse_pad_leds(&wire).expect("erp frame");
        assert_eq!(pads[0][0], led_color([10, 20, 30]));
        assert_eq!(pads[1][7], led_color([1, 2, 3]));
        wire[0] = BUS_MIXER;
        assert!(parse_pad_leds(&wire).is_none());
    }

    #[test]
    fn led_bytes_are_linear_light() {
        // Off stays off, full stays full, half drive is brighter than half on the
        // screen, and the deck program's dimmed HOT CUE white (its lit white
        // scaled by one factor) is a visible gray instead of nearly black.
        let neutral = [255, 255, 255];
        assert_eq!(led_color([0, 0, 0]), Color32::BLACK);
        assert_eq!(led_color_with([255, 255, 255], neutral), Color32::WHITE);
        assert_eq!(led_color_with([0x80, 0, 0], neutral).r(), 188);
        let dim = led_color_with([0x07, 0x0c, 0x07], WHITE_PADMODE);
        assert!(dim.g() > 40 && dim.r().abs_diff(dim.g()) <= 12);
    }

    #[test]
    fn levels_come_from_their_own_bus() {
        let mut wire = [0u8; LED_WIRE_SIZE];
        wire[0] = BUS_LEVELS;
        wire[5..9].copy_from_slice(&0.5f32.to_le_bytes()); // deck 1 left peak
        let rms3 = 1 + 4 + (LEVEL_CHANNELS + 3) * 4; // deck 2 right RMS
        wire[rms3..rms3 + 4].copy_from_slice(&0.25f32.to_le_bytes());
        let l = parse_levels(&wire).unwrap();
        assert_eq!((l.peak[0], l.peak[1], l.rms[3]), (0.5, 0.0, 0.25));
        wire[0] = BUS_ERP;
        assert!(parse_levels(&wire).is_none());
    }

    /// One jog display message, as the deck program frames it.
    fn msg(display: u8, seq: u16, id: u8, index: u16, total: u16, data: &[u8]) -> Frame {
        let mut f = [0u8; LED_FRAME_SIZE];
        f[MSG_PRESENT] = 1;
        f[MSG_SEQ..MSG_SEQ + 2].copy_from_slice(&seq.to_le_bytes());
        f[MSG_DISPLAY] = display;
        f[MSG_ID] = id;
        f[MSG_INDEX..MSG_INDEX + 2].copy_from_slice(&index.to_le_bytes());
        f[MSG_TOTAL..MSG_TOTAL + 2].copy_from_slice(&total.to_le_bytes());
        f[MSG_DATA..MSG_DATA + data.len()].copy_from_slice(data);
        f
    }

    /// The frames that carry `image` to `display`, split as the deck program
    /// splits it, each sent twice (frames repeat until the next message).
    fn art_frames(display: u8, image: &[u8]) -> Vec<Frame> {
        let mut data = vec![
            image.len() as u8,
            (image.len() >> 8) as u8,
            (image.len() >> 16) as u8,
        ];
        data.extend_from_slice(image);
        let pieces: Vec<&[u8]> = data.chunks(MSG_DATA_LEN).collect();
        let n = pieces.len() as u16;
        pieces
            .iter()
            .enumerate()
            .flat_map(|(k, piece)| {
                let f = msg(display, 100 + 2 * k as u16, MSG_ART, k as u16 + 1, n, piece);
                [f, f]
            })
            .collect()
    }

    #[test]
    fn the_jog_image_is_put_back_together_for_either_display() {
        let image: Vec<u8> = (0..500u32).map(|i| (i * 7) as u8).collect();
        for display in DISPLAYS {
            let mut jog = JogDisplay::default();
            let mut whole = None;
            for f in &art_frames(display, &image) {
                // Other deck frames come between the pieces and change nothing.
                assert!(jog.feed(&SPI5_IDLE).is_none());
                whole = jog.feed(f).or(whole);
            }
            assert_eq!(whole, Some(JogEvent::Art(image.clone())));
        }
    }

    #[test]
    fn a_load_is_reported_once_with_its_length() {
        let mut jog = JogDisplay::default();
        // As captured: Damager, 3:44.614.
        let f = msg(0x10, 7, MSG_LOAD, 1, 1, &[0x00, 0x66, 0x6d, 0x03, 0x0a]);
        assert_eq!(jog.feed(&f), Some(JogEvent::Load(224_614)));
        assert_eq!(jog.feed(&f), None);
    }

    #[test]
    fn the_overview_survives_a_load_between_its_pieces() {
        // As captured: the first record of a real overview, after its 0x90.
        let mut data = vec![0x90, 0x29, 0xfa, 0, 0, 0x1c, 0x08, 0];
        data.extend_from_slice(&[0xc9, 0xfe, 0, 0, 0x1c, 0x08, 0]);
        data.resize(2 * MSG_DATA_LEN, 0);
        let mut jog = JogDisplay::default();
        assert!(jog
            .feed(&msg(0x20, 1, MSG_OVERVIEW, 1, 2, &data[..MSG_DATA_LEN]))
            .is_none());
        assert!(jog.feed(&msg(0x20, 2, MSG_LOAD, 1, 1, &[])).is_some());
        let Some(JogEvent::Overview(cols)) =
            jog.feed(&msg(0x20, 3, MSG_OVERVIEW, 2, 2, &data[MSG_DATA_LEN..]))
        else {
            panic!("no overview");
        };
        assert_eq!(cols.len(), 2);
        // 0xfa29: red 31, green 17, blue 9.
        assert_eq!(cols[0].color, Color32::from_rgb(255, 68, 74));
        assert_eq!((cols[0].height, cols[0].inner), (0x1c, 0x08));
    }

    #[test]
    fn the_elapsed_time_is_read_from_its_own_deck_record() {
        let mut f = [0u8; LED_FRAME_SIZE];
        f[RECORD_ID] = RECORD_DECK_1;
        f[RECORD_ELAPSED..RECORD_ELAPSED + 4].copy_from_slice(&[0x01, 0x02, 0xef, 0x00]);
        assert_eq!(elapsed_ms(&f, 0), Some(62_239));
        assert_eq!(elapsed_ms(&f, 1), None);
        // No track loaded.
        f[RECORD_ELAPSED..RECORD_ELAPSED + 4].copy_from_slice(&[0xbf, 0xff, 0xff, 0xff]);
        assert_eq!(elapsed_ms(&f, 0), None);
    }

    #[test]
    fn a_missing_piece_drops_the_jog_image() {
        let image = vec![9u8; 300];
        let frames = art_frames(0x20, &image);
        let mut jog = JogDisplay::default();
        let got: Vec<_> = frames
            .iter()
            .enumerate()
            .filter(|&(k, _)| k / 2 != 2)
            .filter_map(|(_, f)| jog.feed(f))
            .collect();
        assert!(got.is_empty());
        // The next whole image still comes through.
        let mut again = art_frames(0x20, &image);
        for f in &mut again {
            f[MSG_SEQ] = f[MSG_SEQ].wrapping_add(1); // new messages, new numbers
        }
        assert_eq!(
            again.iter().filter_map(|f| jog.feed(f)).last(),
            Some(JogEvent::Art(image))
        );
    }

    #[test]
    fn a_mixer_position_reads_back() {
        let mut f = SPI3_IDLE;
        for (mx, at) in MX_AT.iter().enumerate() {
            set_mx(&mut f, mx, 0.25);
            let lsb = if at.1.is_some() { 1.0 } else { 4.0 } / 1023.0;
            assert!((get_mx(&f, mx) - 0.25).abs() <= lsb, "control {mx}");
        }
        // Setting one control leaves the others' low bits alone.
        set_mx(&mut f, MX_CH1_FADER, 1.0);
        set_mx(&mut f, MX_CH4_FADER, 0.0);
        assert_eq!(get_mx(&f, MX_CH1_FADER), 1.0);
        assert!((get_mx(&f, MX_CH2_FADER) - 0.25).abs() < 1.0 / 1023.0);
    }

    #[test]
    fn the_first_mixer_frame_has_faders_and_master_at_the_middle() {
        let f = initial_mixer_frame();
        let raw = |hi: usize, lo: usize, shift: u8| {
            u16::from(f[hi]) << 2 | u16::from((f[lo] >> shift) & 3)
        };
        // Channel faders 1-4 (0x0a-0x0d, low bits 0x0e) and the master group's LEVEL.
        assert_eq!(&f[0x0a..=0x0e], &[0x80, 0x80, 0x80, 0x80, 0x00]);
        assert_eq!(raw(0x38, 0x3c, 6), 512);
        // Crossfader and the four TRIMs at their centers too.
        assert_eq!(raw(0x0f, 0x10, 0), 512);
        for g in 0..4 {
            assert_eq!(raw(0x24 + 5 * g, 0x28 + 5 * g, 6), 512, "trim {}", g + 1);
        }
        // Control: everything else is the idle frame, byte for byte, apart from
        // the low-bits bytes those controls share (master HI/MID/LOW keep their
        // own low bits).
        let touched = [
            0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x24, 0x28, 0x29, 0x2d, 0x2e, 0x32, 0x33,
            0x37, 0x38, 0x3c,
        ];
        for i in 0..f.len() {
            if !touched.contains(&i) {
                assert_eq!(f[i], SPI3_IDLE[i], "byte {i:#04x}");
            }
        }
        assert_eq!(f[0x3c] & 0x3f, SPI3_IDLE[0x3c] & 0x3f);
        assert_eq!(f[0x28] & 0x3f, SPI3_IDLE[0x28] & 0x3f);
        // And the idle frame itself has them down, so the test can tell.
        assert_eq!(&SPI3_IDLE[0x0a..=0x0d], &[0, 0, 0, 0]);
        assert_eq!(SPI3_IDLE[0x38], 0);
    }

    #[test]
    fn the_mixer_levels_az_drive_keeps_are_the_analog_controls_only() {
        let f = initial_mixer_frame();
        let lv = mixer_levels(&f);
        let at: Vec<usize> = lv.iter().map(|&(i, _)| i).collect();
        for b in [0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x38, 0x24, 0x2e, 0x33] {
            assert!(at.contains(&b), "level byte {b:#x} missing");
        }
        // the press latch, the buttons and the browse knob counter are not levels
        for b in [0x06, 0x20, 0x21, 0x22, 0x16, 0x1d, 0x1e] {
            assert!(!at.contains(&b), "byte {b:#x} is not a level");
        }
        assert!(
            lv.iter().any(|&(i, v)| i == 0x38 && v == 0x80),
            "master at the middle"
        );
    }

    #[test]
    fn a_byte_map_round_trips_and_keeps_az_drives_other_keys() {
        use crate::config::parse_byte_map;
        assert_eq!(
            parse_byte_map(r#"{"11": 128, "22": 7}"#),
            vec![(11, 128), (22, 7)]
        );
        assert_eq!(parse_byte_map("{}"), vec![]);
        assert_eq!(parse_byte_map("not json"), vec![]);
    }

    #[test]
    fn the_latest_frame_of_each_bus_is_kept_for_a_reconnect() {
        let mut latest = LatestFrames::default();
        assert!(latest.messages().is_empty());
        let mut later = initial_mixer_frame();
        later[0x0a] = 0xff;
        for (bus, frame) in [
            (BUS_MIXER, initial_mixer_frame()),
            (BUS_ERP, ERP_IDLE),
            (BUS_MIXER, later),
        ] {
            latest.note(&CtrlMsg { bus, frame });
        }
        let m = latest.messages();
        assert_eq!(
            m.iter().map(|m| m.bus).collect::<Vec<_>>(),
            vec![BUS_ERP, BUS_MIXER]
        );
        assert_eq!(m[1].frame, later, "the newest mixer frame, not the first");
    }

    #[test]
    fn a_pad_mode_button_reads_its_own_color_and_none_when_off() {
        let mut erp = [0u8; LED_FRAME_SIZE];
        // deck 1, mode button 1 (BEAT LOOP); deck 2, mode button 3 (BEAT JUMP)
        erp[0x0e + 4..0x0e + 4 + 3].copy_from_slice(&[0xff, 0x80, 0x00]);
        erp[0x0e + 0x38 + 12..0x0e + 0x38 + 12 + 3].copy_from_slice(&[0x00, 0x00, 0xff]);
        assert_eq!(
            padmode_led(&erp, 0, 1),
            Some(led_color_with([0xff, 0x80, 0x00], WHITE_PADMODE))
        );
        assert_eq!(
            padmode_led(&erp, 1, 3),
            Some(led_color_with([0x00, 0x00, 0xff], WHITE_PADMODE))
        );
        assert_eq!(padmode_led(&erp, 0, 0), None);
        assert_eq!(padmode_led(&erp, 1, 1), None);
        // the pads' own colors start where the mode buttons end
        assert_eq!(0x0e + 4 * LED_PAD_STRIDE, LED_PAD_OFF_DECK1);
    }

    #[test]
    fn the_decks_whites_show_white_and_its_colors_keep_their_hue() {
        let white = Color32::from_rgb(255, 255, 255);
        assert_eq!(led_color_with(WHITE_PADMODE, WHITE_PADMODE), white);
        assert_eq!(led_color(WHITE_PAD), white);
        // dimmed white stays a neutral gray
        let dim = led_color_with([0x07, 0x0c, 0x07], WHITE_PADMODE);
        let (r, g, b) = (i32::from(dim.r()), i32::from(dim.g()), i32::from(dim.b()));
        assert!((r - g).abs() <= 12 && (g - b).abs() <= 12, "{dim:?}");
        // a hot cue's green, a beat jump's orange and a pad's blue keep their hue
        let green = led_color([0x10, 0xff, 0x00]);
        assert!(green.g() == 255 && green.r() < 100 && green.b() == 0);
        let orange = led_color([0xcb, 0x4c, 0x00]);
        assert!(orange.r() == 255 && orange.g() > 100 && orange.g() < 220 && orange.b() == 0);
        assert_eq!(led_color([0x00, 0x00, 0xff]), Color32::from_rgb(0, 0, 255));
    }

    #[test]
    fn each_knob_owns_its_low_bits() {
        // Shifts 6, 4, 2, 0 follow the high bytes of a low-bits group in order;
        // the crossfader has its low-bits byte (0x10) to itself.
        let mut all: Vec<(usize, usize, u8)> = MX10.iter().map(|k| (k.1, k.2, k.3)).collect();
        all.extend(
            MX_AT
                .iter()
                .filter_map(|&(hi, lo)| lo.map(|(l, s)| (hi, l, s))),
        );
        for &(hi, lo, shift) in &all {
            let base = all.iter().filter(|k| k.1 == lo).map(|k| k.0).min().unwrap();
            if lo != 0x10 {
                assert_eq!(shift, 6 - 2 * (hi - base) as u8, "high byte {hi:#x}");
            }
            let twins = all.iter().filter(|k| k.1 == lo && k.2 == shift).count();
            assert_eq!(twins, 1, "low bits {lo:#x}>>{shift} shared");
        }
        // A value written is read back, and the neighbors' low bits survive.
        let mut f = SPI3_IDLE;
        f[0x50] = 0xff;
        set10(&mut f, 0x4d, 0x50, 4, 0x2c5);
        assert_eq!(get10(&f, 0x4d, 0x50, 4), 0x2c5);
        assert_eq!(f[0x50] & 0xcf, 0xcf);
    }

    #[test]
    fn mixer_buttons_have_distinct_bits() {
        let mut seen = std::collections::HashSet::new();
        let others = [MIXER_SETTINGS, KNOB_PUSH, (MX_FXSEL_BYTE, 0x01)];
        for b in MIXER_BUTTONS {
            assert!(b.mask.count_ones() == 1, "{}", b.name);
            assert!(seen.insert((b.byte, b.mask)), "{} twice", b.name);
            assert!(!others.contains(&(b.byte, b.mask)), "{}", b.name);
            // Never the Beat FX SELECT nibble, the strobe or a knob byte.
            assert!(!(b.byte == MX_FXSEL_BYTE && b.mask >= 0x10), "{}", b.name);
            assert!((0x14..=0x18).contains(&b.byte), "{}", b.name);
        }
        assert_eq!(MIXER_BUTTONS.len(), 31);
    }
}
