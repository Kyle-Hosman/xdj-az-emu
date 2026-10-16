// SPDX-License-Identifier: GPL-3.0-only
//! Starting and stopping the emulator. The app runs QEMU headless through the
//! platform's boot script, so there is one window to open, and attaches to an
//! emulator that is already running instead of starting a second one.

use crate::config::repo_root;
#[cfg(unix)]
use crate::config::sock_dir;

/// The emulator this app started: on unix the supervisor process that owns QEMU
/// (see [`supervise`]), its pid on Windows (az-boot-win.ps1 starts QEMU detached
/// and returns).
#[cfg(unix)]
pub type Launched = std::process::Child;
#[cfg(windows)]
pub type Launched = u32;

/// Start the emulator unless one is running. None = attached to a running one
/// (or the start failed; the reason is on stderr and, on unix, in qemu.log).
#[cfg(unix)]
pub fn start_if_needed() -> Option<Launched> {
    // The boot script `exec`s QEMU, so its own name is gone from the process
    // list once QEMU runs: look for QEMU's command line instead.
    let running = std::process::Command::new("pgrep")
        .args(["-f", "qemu-system-aarch64.*az-mnt.img"])
        .output()
        .is_ok_and(|o| o.status.success());
    if running {
        eprintln!("az-preview: found a running emulator, attaching to it");
        return None;
    }
    // The boot script's and QEMU's output go to qemu.log, whose last line the
    // window shows if the emulator exits (see `failure`).
    let _ = std::fs::create_dir_all(sock_dir());
    let log = std::fs::File::create(sock_dir().join("qemu.log"));
    let (out, err) = match log.and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((a, b)) => (a.into(), b.into()),
        Err(_) => (std::process::Stdio::null(), std::process::Stdio::null()),
    };
    // QEMU runs under a supervisor (this program again) that stops it when this
    // process ends, however it ends: the supervisor's stdin is a pipe only this
    // process holds, so the system closes it on any exit, kill or crash included.
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("az-preview: could not start the emulator: {e}");
            return None;
        }
    };
    std::process::Command::new(exe)
        .arg(SUPERVISE_ARG)
        .arg(repo_root().join("tools").join("az-boot-m1.sh"))
        .stdin(std::process::Stdio::piped())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|e| eprintln!("az-preview: could not start the emulator: {e}"))
        .ok()
}

/// The argument that runs this program as QEMU's supervisor instead of the app.
#[cfg(unix)]
pub const SUPERVISE_ARG: &str = "--supervise-qemu";

/// Run as QEMU's supervisor: start the boot script (which `exec`s QEMU, so the
/// script's child is QEMU itself) and wait. When QEMU exits, exit with its code.
/// When stdin reaches end of file, the app that started this is gone: stop QEMU
/// as [`stop_child`] does. Being QEMU's parent, this process is the only one that
/// signals it, and its pid cannot be reused by another program until it is
/// reaped here.
#[cfg(unix)]
pub fn supervise(script: &std::ffi::OsStr) -> i32 {
    let qemu = std::process::Command::new("bash")
        .arg(script)
        .env("AZ_DISPLAY", "none")
        .stdin(std::process::Stdio::null())
        .spawn();
    let qemu = match qemu {
        Ok(c) => c,
        Err(e) => {
            eprintln!("az-preview: could not start the emulator: {e}");
            return 1;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        let _ = tx.send(());
    });
    supervise_child(qemu, &rx, QUIT_GRACE)
}

/// [`supervise`] with the child and the end-of-input signal passed in. Returns
/// the child's exit code if it exits by itself, 0 once it was stopped.
#[cfg(unix)]
fn supervise_child(
    mut child: std::process::Child,
    app_gone: &std::sync::mpsc::Receiver<()>,
    grace: std::time::Duration,
) -> i32 {
    use std::sync::mpsc::RecvTimeoutError;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(1),
            Ok(None) => {}
            Err(_) => return 1,
        }
        match app_gone.recv_timeout(std::time::Duration::from_millis(100)) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    stop_child(&mut child, grace);
    0
}

/// Stop a child process: SIGTERM, which QEMU handles like a QMP `quit` (it
/// flushes its disk caches to the image files and exits), then SIGKILL if it is
/// still running after `grace`. The child is not reaped before the signals are
/// sent, so its pid still names it.
#[cfg(unix)]
fn stop_child(child: &mut std::process::Child, grace: std::time::Duration) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    if matches!(child.try_wait(), Ok(None)) {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: kill(2) on the pid of an unreaped child of this process.
            unsafe { kill(pid, SIGTERM) };
        }
    }
    if !wait_exit(child, grace) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Wait up to `grace` for `child` to exit. True if it has.
#[cfg(unix)]
fn wait_exit(child: &mut std::process::Child, grace: std::time::Duration) -> bool {
    let until = std::time::Instant::now() + grace;
    loop {
        if !matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Windows: the same through tools\az-boot-win.ps1, which starts QEMU detached,
/// prints "qemu pid N" and returns. The script also installs the current mods.
#[cfg(windows)]
pub fn start_if_needed() -> Option<Launched> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW: no console flash
    let running = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq qemu-system-aarch64.exe", "/NH"])
        .creation_flags(NO_WINDOW)
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("qemu-system-aarch64"));
    if running {
        eprintln!("az-preview: found a running emulator, attaching to it");
        return None;
    }
    let script = repo_root().join("tools").join("az-boot-win.ps1");
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .args(["-Display", "none"])
        .creation_flags(NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| eprintln!("az-preview: could not start the emulator: {e}"))
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = text
        .split_whitespace()
        .skip_while(|w| *w != "pid")
        .nth(1)
        .and_then(|n| n.parse().ok());
    match pid {
        Some(pid) => win::stop_with_this_app(pid),
        None => {
            let err = String::from_utf8_lossy(&out.stderr);
            eprintln!(
                "az-preview: {} did not start QEMU:\n{text}{err}",
                script.display()
            );
            let _ = START_ERROR.set(script_error(&err, &text));
        }
    }
    pid
}

/// Windows: why the boot script did not start QEMU, if it did not.
#[cfg(windows)]
static START_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The boot script's own reason, for the window: PowerShell wraps an error over
/// several lines and follows it with "At line:" and "+ " lines, which are left out.
#[cfg_attr(not(windows), allow(dead_code))]
fn script_error(stderr: &str, stdout: &str) -> String {
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let why: Vec<&str> = source
        .lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty() && !l.starts_with("At ") && !l.starts_with("+ "))
        .collect();
    if why.is_empty() {
        "no output from the boot script".to_string()
    } else {
        why.join(" ")
    }
}

/// How long QEMU gets to quit on its own (flushing its disk caches) before it is
/// killed.
const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(6);

/// Stop the emulator this app started. The caller has asked QEMU to quit over
/// QMP; closing the supervisor's stdin makes it stop QEMU too (and kill it after
/// [`QUIT_GRACE`]). This waits for the supervisor a little longer than that. The
/// supervisor itself is never killed, since QEMU would then outlive it.
#[cfg(unix)]
pub fn stop(child: &mut Launched) {
    drop(child.stdin.take());
    wait_exit(child, QUIT_GRACE + std::time::Duration::from_secs(2));
}
#[cfg(windows)]
pub fn stop(pid: &mut Launched) {
    if win::wait_exit(*pid, QUIT_GRACE) {
        return;
    }
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status();
}

/// Another AZ-EMU (this program, by its file name) is running: then this one
/// neither starts nor attaches to an emulator, since QEMU takes one app at a
/// time and two starting at once could both boot the same disk image.
pub fn other_instance() -> bool {
    let Some(name) = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
    else {
        return false;
    };
    let me = std::process::id().to_string();
    #[cfg(windows)]
    let listed = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000)
            .output()
    };
    #[cfg(unix)]
    let listed = std::process::Command::new("pgrep")
        .args(["-x", &name])
        .output();
    let Ok(out) = listed else { return false };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        // tasklist: "name","pid",...; pgrep: one pid per line
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(|s| s.trim_matches('"')).collect();
            f.get(1).or(f.first()).map(|s| s.to_string())
        })
        .any(|pid| !pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit()) && pid != me)
}

/// Windows: the launched QEMU is not this app's child (the boot script starts it
/// detached), so nothing stops it if the app ends without `on_exit` (a crash, a
/// kill). It is put in a job that the system closes with this process, and the
/// job kills what it holds when closed.
#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::time::Duration;

    type Handle = *mut c_void;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const WAIT_OBJECT_0: u32 = 0;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimit {
        per_process_user_time: i64,
        per_job_user_time: i64,
        limit_flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn CloseHandle(h: Handle) -> i32;
    }

    /// Tie QEMU `pid` to this process's lifetime. The job handle is left open
    /// on purpose: the system closes it when this process ends, however it ends.
    pub fn stop_with_this_app(pid: u32) {
        // SAFETY: plain Win32 calls on handles this function owns; the limit
        // struct matches JOBOBJECT_EXTENDED_LIMIT_INFORMATION's layout.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                eprintln!("az-preview: no job object; QEMU outlives a crash of this app");
                return;
            }
            let info = ExtendedLimit {
                basic: BasicLimit {
                    limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };
            let set = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&info as *const ExtendedLimit).cast(),
                std::mem::size_of::<ExtendedLimit>() as u32,
            );
            let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if set == 0 || proc.is_null() || AssignProcessToJobObject(job, proc) == 0 {
                eprintln!("az-preview: could not tie QEMU {pid} to this app; it outlives a crash");
            }
            if !proc.is_null() {
                CloseHandle(proc);
            }
        }
    }

    /// Wait up to `grace` for process `pid` to exit. True if it has (or is gone).
    pub fn wait_exit(pid: u32, grace: Duration) -> bool {
        // SAFETY: a process handle opened and closed here.
        unsafe {
            let h = OpenProcess(SYNCHRONIZE, 0, pid);
            if h.is_null() {
                return true;
            }
            let r = WaitForSingleObject(h, grace.as_millis() as u32);
            CloseHandle(h);
            r == WAIT_OBJECT_0
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_limit_struct_has_the_win32_layout() {
            // JOBOBJECT_EXTENDED_LIMIT_INFORMATION is 144 bytes on 64-bit Windows,
            // with LimitFlags 16 bytes in.
            assert_eq!(std::mem::size_of::<super::ExtendedLimit>(), 144);
            assert_eq!(std::mem::offset_of!(super::BasicLimit, limit_flags), 16);
        }
    }
}

/// If the emulator this app started has exited: the last line of its log (the
/// boot script's preflight message or QEMU's error).
#[cfg(unix)]
pub fn failure(launched: &mut Option<Launched>) -> Option<String> {
    let child = launched.as_mut()?;
    if !matches!(child.try_wait(), Ok(Some(_))) {
        return None;
    }
    let log = std::fs::read_to_string(sock_dir().join("qemu.log")).unwrap_or_default();
    let last = log
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(no output)");
    Some(last.to_string())
}
/// Windows: the script has returned by the time the window opens, so there is
/// no child to watch: its error, if it did not start QEMU.
#[cfg(windows)]
pub fn failure(_launched: &mut Option<Launched>) -> Option<String> {
    START_ERROR.get().cloned()
}

#[cfg(all(test, unix))]
mod supervisor_tests {
    use super::supervise_child;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> std::process::Child {
        Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn running(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn a_child_that_exits_passes_its_code_through() {
        let (_tx, rx) = channel();
        assert_eq!(
            supervise_child(sh("exit 3"), &rx, Duration::from_secs(5)),
            3
        );
    }

    #[test]
    fn the_app_going_away_stops_the_child_with_sigterm() {
        let child = sh("exec sleep 30");
        let pid = child.id();
        let (tx, rx) = channel();
        drop(tx);
        let t = Instant::now();
        assert_eq!(supervise_child(child, &rx, Duration::from_secs(5)), 0);
        assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
        assert!(!running(pid));
    }

    #[test]
    fn a_child_that_ignores_sigterm_is_killed_after_the_grace() {
        let child = sh("trap '' TERM; while :; do sleep 0.05; done");
        let pid = child.id();
        std::thread::sleep(Duration::from_millis(200));
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        let t = Instant::now();
        assert_eq!(supervise_child(child, &rx, Duration::from_millis(500)), 0);
        assert!(
            t.elapsed() >= Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        assert!(!running(pid));
    }
}

#[cfg(test)]
mod tests {
    use super::script_error;

    #[test]
    fn a_wrapped_powershell_error_is_joined_without_its_trailer() {
        let err = "\r\nC:\\x\\az-boot-win.ps1 : QEMU not found: 'C:\\q.exe'. Set AZ_QEMU (or\r\n\
                   install QEMU).\r\nAt line:1 char:1\r\n+ & 'C:\\x\\az-boot-win.ps1'\r\n";
        assert_eq!(
            script_error(err, ""),
            "C:\\x\\az-boot-win.ps1 : QEMU not found: 'C:\\q.exe'. Set AZ_QEMU (or install QEMU)."
        );
        assert_eq!(script_error("", "only stdout\n"), "only stdout");
        assert_eq!(script_error("", ""), "no output from the boot script");
    }
}
