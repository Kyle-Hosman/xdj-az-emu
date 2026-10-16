// SPDX-License-Identifier: GPL-3.0-only
//! The emulated deck's screen over QMP: live frames from `screendump`, and
//! taps injected as absolute pointer events.

use crate::config::{scratch, QMP_ADDR};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

/// The deck's native screen size.
pub const LCD_W: f32 = 1280.0;
pub const LCD_H: f32 = 800.0;

/// A request from the UI.
pub enum EmuCmd {
    /// Tap at screen-normalized coordinates (0..1, 0..1).
    Tap(f32, f32),
    /// Ask QEMU to quit: it flushes its disk caches to the image files and exits.
    Quit,
}

/// Keep a QMP connection, poll the screen and send each frame to the UI, and
/// inject the taps it asks for. Reconnects when the emulator restarts; returns
/// when the UI is gone.
pub fn emu_thread(cmd_rx: Receiver<EmuCmd>, frame_tx: Sender<egui::ColorImage>) {
    // The path goes into a JSON string unescaped; QEMU and std::fs both take
    // forward slashes in Windows drive-letter paths.
    let path = scratch("az_lcd_live.ppm")
        .to_string_lossy()
        .replace('\\', "/");
    loop {
        let Some((mut wr, mut rd)) = connect() else {
            thread::sleep(Duration::from_millis(700));
            continue;
        };
        loop {
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    EmuCmd::Tap(x, y) => {
                        if tap(&mut wr, &mut rd, x, y).is_none() {
                            break;
                        }
                    }
                    EmuCmd::Quit => {
                        let _ = wr.write_all(b"{\"execute\":\"quit\"}\r\n");
                        let _ = read_reply(&mut rd);
                        return;
                    }
                }
            }
            let sd = format!(
                "{{\"execute\":\"screendump\",\"arguments\":{{\"filename\":\"{path}\"}}}}\r\n"
            );
            if wr.write_all(sd.as_bytes()).is_err() || read_reply(&mut rd).is_none() {
                break;
            }
            if let Some(img) = std::fs::read(&path).ok().and_then(|d| decode_ppm(&d)) {
                if frame_tx.send(img).is_err() {
                    return;
                }
            }
            // A screendump costs a few milliseconds, so this sets the frame rate.
            thread::sleep(Duration::from_millis(40));
        }
    }
}

/// Connect and finish the QMP handshake.
fn connect() -> Option<(TcpStream, BufReader<TcpStream>)> {
    let stream = TcpStream::connect(QMP_ADDR).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut wr = stream.try_clone().ok()?;
    let mut rd = BufReader::new(stream);
    let mut greeting = String::new();
    rd.read_line(&mut greeting).ok()?;
    wr.write_all(b"{\"execute\":\"qmp_capabilities\"}\r\n")
        .ok()?;
    read_reply(&mut rd)?;
    Some((wr, rd))
}

/// A tap: move the absolute pointer, press, release.
fn tap(wr: &mut TcpStream, rd: &mut impl BufRead, x: f32, y: f32) -> Option<()> {
    let ax = (x.clamp(0.0, 1.0) * 32767.0).round() as i32;
    let ay = (y.clamp(0.0, 1.0) * 32767.0).round() as i32;
    let down = format!(
        "{{\"execute\":\"input-send-event\",\"arguments\":{{\"events\":[\
         {{\"type\":\"abs\",\"data\":{{\"axis\":\"x\",\"value\":{ax}}}}},\
         {{\"type\":\"abs\",\"data\":{{\"axis\":\"y\",\"value\":{ay}}}}},\
         {{\"type\":\"btn\",\"data\":{{\"button\":\"left\",\"down\":true}}}}]}}}}\r\n"
    );
    let up = "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[\
              {\"type\":\"btn\",\"data\":{\"button\":\"left\",\"down\":false}}]}}\r\n";
    wr.write_all(down.as_bytes()).ok()?;
    read_reply(rd)?;
    wr.write_all(up.as_bytes()).ok()?;
    read_reply(rd)
}

/// Read QMP lines up to a command's reply, skipping async events. None on an
/// I/O error or a closed connection.
fn read_reply(rd: &mut impl BufRead) -> Option<()> {
    loop {
        let mut line = String::new();
        if rd.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line.contains("\"return\"") || line.contains("\"error\"") {
            return Some(());
        }
    }
}

/// Decode a binary PPM (P6, 8-bit) into an image at full resolution. Returns
/// None for a malformed file and for a near-black frame: the deck program
/// briefly clears the screen between views, and keeping the last good frame
/// avoids a flash.
pub fn decode_ppm(data: &[u8]) -> Option<egui::ColorImage> {
    let rest = data.strip_prefix(b"P6")?;
    let mut fields = Vec::with_capacity(3);
    let mut i = 0;
    while fields.len() < 3 {
        while rest.get(i)?.is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while rest.get(i)?.is_ascii_digit() {
            i += 1;
        }
        fields.push(
            std::str::from_utf8(&rest[start..i])
                .ok()?
                .parse::<usize>()
                .ok()?,
        );
    }
    let (w, h) = (fields[0], fields[1]);
    let pixels = rest.get(i + 1..i + 1 + w * h * 3)?; // one whitespace after maxval
    let sum: u64 = pixels.iter().map(|&b| u64::from(b)).sum();
    if sum < 3 * pixels.len() as u64 {
        return None; // mean below 3 of 255
    }
    Some(egui::ColorImage::from_rgb([w, h], pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppm(w: usize, h: usize, v: u8) -> Vec<u8> {
        let mut d = format!("P6\n{w} {h}\n255\n").into_bytes();
        d.extend(vec![v; w * h * 3]);
        d
    }

    #[test]
    fn decodes_ppm_and_skips_black_frames() {
        let img = decode_ppm(&ppm(4, 2, 200)).expect("decodes");
        assert_eq!(img.size, [4, 2]);
        assert_eq!(img.pixels[7], egui::Color32::from_rgb(200, 200, 200));
        assert!(decode_ppm(&ppm(4, 2, 0)).is_none(), "black frame");
        let mut short = ppm(4, 2, 200);
        short.pop();
        assert!(decode_ppm(&short).is_none(), "truncated");
        assert!(decode_ppm(b"P5\n1 1\n255\n\0").is_none(), "not P6");
    }
}
