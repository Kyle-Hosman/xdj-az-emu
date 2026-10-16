// SPDX-License-Identifier: GPL-3.0-only
//! Where things live: the repository, the boot script's socket folder, scratch
//! files, and the browse knob counter shared with `tools/az-drive.py`.

use std::path::PathBuf;

/// The QMP port that belongs to this app. Port 4446 is left for automation.
pub const QMP_ADDR: &str = "127.0.0.1:4445";

/// The boot script's socket folder: `/tmp/az-emu-m1` (az-boot-m1.sh, not
/// `$TMPDIR`), or `%TEMP%\az-emu-win` (az-boot-win.ps1).
#[cfg(unix)]
pub fn sock_dir() -> PathBuf {
    PathBuf::from("/tmp/az-emu-m1")
}
#[cfg(windows)]
pub fn sock_dir() -> PathBuf {
    // %TEMP%, as the boot script and the Python tools use; temp_dir() reads %TMP%
    // first, and the two can differ
    std::env::var_os("TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("az-emu-win")
}

/// A scratch file (the live screen frame, debug logs).
#[cfg(unix)]
pub fn scratch(name: &str) -> PathBuf {
    PathBuf::from("/tmp").join(name)
}
#[cfg(windows)]
pub fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

/// The repository the app belongs to, found at run time (no build-machine path
/// is compiled in): `AZ_EMU_REPO`, else the checkout the executable sits in
/// (AZ-EMU.app and target/ both live inside it), else the current directory.
pub fn repo_root() -> PathBuf {
    if let Some(r) = std::env::var_os("AZ_EMU_REPO") {
        return PathBuf::from(r);
    }
    let script = if cfg!(windows) {
        "az-boot-win.ps1"
    } else {
        "az-boot-m1.sh"
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(r) = exe
            .ancestors()
            .find(|d| d.join("tools").join(script).is_file())
        {
            return r.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_default()
}

/// The browse knob counter (mixer byte 34), kept in `<sock_dir>/knob.ctr` so
/// this app and `tools/az-drive.py` never disagree about where the knob is.
/// The boot scripts delete the file, since every boot starts the buses idle.
pub fn knob_ctr_load() -> Option<u8> {
    let s = std::fs::read_to_string(sock_dir().join("knob.ctr")).ok()?;
    s.trim().parse::<u32>().ok().map(|v| (v & 0xff) as u8)
}

pub fn knob_ctr_store(v: u8) {
    let _ = std::fs::write(sock_dir().join("knob.ctr"), v.to_string());
}

/// The mixer's analog levels (faders, crossfader, trims, EQ, master, ...), kept in
/// `<sock_dir>/mixer.state` as `{"byte": value}` beside the knob counter, so
/// `tools/az-drive.py` builds its mixer frames on the positions this app set
/// instead of the idle frame's (faders and master down, a silent mix). Other bytes
/// az-drive keeps there are left as they are. The boot scripts delete the file.
pub fn mixer_state_store(levels: &[(usize, u8)]) {
    let path = sock_dir().join("mixer.state");
    let mut kept = std::fs::read_to_string(&path)
        .map(|s| parse_byte_map(&s))
        .unwrap_or_default();
    for &(k, v) in levels {
        match kept.iter_mut().find(|(at, _)| *at == k) {
            Some(e) => e.1 = v,
            None => kept.push((k, v)),
        }
    }
    let body: Vec<String> = kept.iter().map(|(k, v)| format!("\"{k}\": {v}")).collect();
    let tmp = path.with_extension("state.new");
    if std::fs::write(&tmp, format!("{{{}}}", body.join(", "))).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// `{"11": 128, "56": 0}` as az-drive writes it; anything else is skipped.
pub fn parse_byte_map(s: &str) -> Vec<(usize, u8)> {
    s.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            let k = k.trim().trim_matches('"').parse::<usize>().ok()?;
            let v = v.trim().parse::<u32>().ok()?;
            Some((k, (v & 0xff) as u8))
        })
        .collect()
}
