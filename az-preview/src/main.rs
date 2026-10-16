// SPDX-License-Identifier: GPL-3.0-only
//! AZ-EMU: the emulated XDJ-AZ's screen and front panel.
//!
//! Starts the emulator headless (or attaches to a running one) and shows its
//! screen, alone or inside a drawing of the whole front panel whose controls
//! drive the guest's buses.
//!
//! - `cargo run -p az-preview` runs it.
//! - `AZ_FULL=1` opens the whole panel instead of the screen alone;
//!   `AZ_SHOW_HITS=1` outlines every control's hit area.
//! - `AZ_SHOT=out.png` draws the panel once, saves a screenshot and quits
//!   (without starting the emulator).
//! - `AZ_CTRL_LOG=1` and `AZ_LED_DEBUG=1` log the bus frames sent and the LED
//!   frames received to scratch files.
//!
//! Modules: `config` (paths), `boot` (starting QEMU), `screen` (QMP: frames and
//! taps), `controls` (the buses' wire format and the control socket), `panel`
//! (drawing), `parts` (the jog, tempo slider and rotaries, from cdj3k-emu), `app`
//! (the window and its mouse handling).

// A GUI app on Windows: no console window beside it when opened from Explorer.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod boot;
mod config;
mod controls;
mod layout;
mod meter;
mod panel;
mod parts;
mod screen;

use std::sync::mpsc::channel;
use std::thread;

fn main() -> eframe::Result<()> {
    #[cfg(unix)]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().is_some_and(|a| a == boot::SUPERVISE_ARG) {
            let script = args.next().unwrap_or_default();
            std::process::exit(boot::supervise(&script));
        }
    }
    let shot_path = std::env::var("AZ_SHOT").ok();
    let (cmd_tx, cmd_rx) = channel();
    let (frame_tx, frame_rx) = channel();
    let (ctrl_tx, ctrl_rx) = channel();
    let (led_tx, led_rx) = channel();
    // A second AZ-EMU would find the emulator's single app connection taken (or,
    // starting at the same moment as the first, boot a second QEMU on the same
    // disk image): it says so and does nothing else.
    let notice = (shot_path.is_none() && boot::other_instance()).then(|| {
        "AZ-EMU is already open.\nThe emulator is shown in that window; close this one.".to_string()
    });
    let launched = if shot_path.is_none() && notice.is_none() {
        thread::spawn(move || screen::emu_thread(cmd_rx, frame_tx));
        thread::spawn(move || controls::ctrl_thread(ctrl_rx, led_tx));
        boot::start_if_needed()
    } else {
        None
    };
    let links = app::Links {
        cmd_tx,
        frame_rx,
        ctrl_tx,
        led_rx,
    };
    let options = eframe::NativeOptions {
        // The startup size is a guess for the screen-only view; App sets the
        // exact one on its first frame. Clamping to the monitor keeps a switch
        // to the full panel from pushing the window off a small screen.
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([screen::LCD_W, screen::LCD_H])
            .with_title("XDJ-AZ")
            .with_clamp_size_to_monitor_size(true),
        ..Default::default()
    };
    eframe::run_native(
        "az-preview",
        options,
        Box::new(move |_cc| Ok(Box::new(app::App::new(links, launched, shot_path, notice)))),
    )
}
