# SPDX-License-Identifier: GPL-3.0-only
# Derived from cdj3k-emu (MIT OR Apache-2.0); see ATTRIBUTION.md.
# az-boot-win.ps1 - the AZ guest on Windows (x86_64 host): az-boot-m1.sh's machine,
# minus what only macOS has. Keep the two in step: every device, the 1280x800
# edid=off screen, the four virtserialports, QMP 4445 (the app's) + 4446
# (automation) and SSH on 2222 are the same.
#
# Differences, and why:
#   -accel tcg,thread=multi  an aarch64 guest on an x86_64 host has no hardware
#                            acceleration: WHPX (Windows Hypervisor Platform) only
#                            runs guests of the host's own architecture, so TCG
#                            (QEMU's translator) is the only option. Turning WHPX
#                            on would change nothing; no reboot or setting needed.
#                            thread=multi runs each guest CPU on its own host thread.
#   -cpu cortex-a72          the deck's own core; under TCG there is no "host" CPU to
#   + nokaslr                pass through. With KASLR on, the kernel turns KPTI on for
#                            an A72 and every user-mode exception flushes TCG's TLB:
#                            measured 0.18x real time. With nokaslr it is the fastest
#                            measured setting (about 0.79x, against 0.70x for -cpu
#                            max) on fewer host cores. KASLR is off in the emulator only.
#   -smp 6                   the DECK's six cores, whatever the host has: x11-only.sh
#                            pins X with `taskset -c 3` and EP147's apl_start.sh
#                            pins too, so with -smp 2 X exits ("taskset: failed to
#                            set pid 0's affinity") and EP147 dies with "Failed to
#                            connect to the X Server".
#   -display sdl             no cocoa. AZ_DISPLAY / -Display none = headless.
#   audio                    -Audio off | speakers | file:<path.wav>, as AZ_AUDIO
#                            on the Mac. speakers is DirectSound (dsound) instead
#                            of CoreAudio; off is wav to NUL.
#   sockets                  AF_UNIX sockets in %TEMP%\az-emu-win (Windows 10
#                            1803+ has them), same names as /tmp/az-emu-m1.
#
# QEMU: the official Windows build (qemu.weilnetz.de), installed or unpacked
# anywhere. Point -Qemu or the AZ_QEMU environment variable at
# qemu-system-aarch64.exe, or put its folder on PATH.
#   powershell -ExecutionPolicy Bypass -File tools\az-boot-win.ps1 [-Display none]
# Starts QEMU DETACHED and returns; stop it with QMP {"execute":"quit"} on 4446.
param(
  [int]$Smp = 6,
  # TCG levers (docs/windows.md): -Cpu picks the emulated core (default cortex-a72, the
  # deck's own; nokaslr is always on the kernel command line, see above); -Tcg adds accel
  # options, e.g. tb-size=512.
  [string]$Cpu = $(if ($env:AZ_CPU) { $env:AZ_CPU } else { 'cortex-a72' }),
  [string]$Tcg = $(if ($env:AZ_TCG) { $env:AZ_TCG } else { '' }),
  [string]$Display = $(if ($env:AZ_DISPLAY) { $env:AZ_DISPLAY } else { 'sdl,show-cursor=on' }),
  # Audio output: off (default) | speakers | file:<path.wav>  (or AZ_AUDIO)
  #   off       the stream is consumed and discarded (wav to NUL). It must be consumed:
  #             with QEMU's "none" backend EP147 renders silence and stems break.
  #   speakers  Windows' default output device (DirectSound).
  #   file      a new WAV in audio-out-captures\, named by date and time.
  #   file:...  a 16-bit stereo WAV of everything the deck plays, for measuring your
  #             mod's output. It grows about 176 KB/s (about 635 MB an hour) while QEMU
  #             runs; tools/az-wav-fix.py fills in its header. Delete it when you are done.
  # AZ_AUDIODEV, when set, is passed to QEMU's -audiodev as-is and overrides -Audio.
  [string]$Audio = $(if ($env:AZ_AUDIO) { $env:AZ_AUDIO } else { 'off' }),
  # Every boot checks that the CURRENT published mods set is installed and installs it
  # if not (tools/az-mods-fetch.py, signature-checked; a no-op when it is current).
  # -NoMods (or AZ_EMU_NOMODS=1) skips that: use it while you test your OWN core build
  # (az-push-mod.py --core), which this would otherwise replace, or for a bench.
  [switch]$NoMods,
  [string]$Qemu = $(if ($env:AZ_QEMU) { $env:AZ_QEMU } else { 'qemu-system-aarch64.exe' })
)
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
$Repo = Split-Path -Parent $PSScriptRoot
$Build = if ($env:AZ_EMU_BUILD) { $env:AZ_EMU_BUILD } else { Join-Path $Repo 'build' }
$Sock = Join-Path $env:TEMP 'az-emu-win'
New-Item -ItemType Directory -Force $Sock | Out-Null
# Every bus starts at its idle frame on a new boot, so the control state that
# az-drive.py and AZ-EMU.app keep here must start from idle too; a stale knob
# counter would turn the first knob frame into a multi-detent jump.
Remove-Item -ErrorAction SilentlyContinue (Join-Path $Sock 'knob.ctr'), (Join-Path $Sock 'deck1.state'), (Join-Path $Sock 'deck2.state'), (Join-Path $Sock 'mixer.state')
$Serial = if ($env:SERIAL_LOG) { $env:SERIAL_LOG } else { Join-Path $Sock 'serial.log' }
$Mem = 4038197248   # the deck's RAM, 64 KiB-aligned
$QemuCmd = Get-Command $Qemu -ErrorAction SilentlyContinue
if (-not $QemuCmd) {
  Write-Error "QEMU not found: '$Qemu'. Set AZ_QEMU (or pass -Qemu) to the full path of qemu-system-aarch64.exe."
  exit 2
}
$Qemu = $QemuCmd.Source
if ($Smp -lt 6) { Write-Warning "-Smp $Smp < 6: the guest pins X and EP147 to deck CPUs that will not exist." }

if ($Audio -eq 'off') {
  $AudioDev = 'wav,id=audio0,path=NUL'
} elseif ($Audio -eq 'speakers') {
  $AudioDev = 'dsound,id=audio0'
} elseif ($Audio -match '^file(:(.*))?$') {
  $Wav = if ($Matches[2]) { $Matches[2] } else { (Get-Date -Format 'yyyy-MM-dd_HH-mm-ss') + '.wav' }
  if (-not [IO.Path]::IsPathRooted($Wav)) { $Wav = Join-Path (Join-Path $Repo 'audio-out-captures') $Wav }
  $Wav = [IO.Path]::GetFullPath($Wav)
  if ($Wav.Contains(',')) {
    Write-Error "-Audio file path cannot contain a comma: $Wav"
    exit 3
  }
  New-Item -ItemType Directory -Force (Split-Path -Parent $Wav) | Out-Null
  Write-Host "az-boot-win: recording audio to $Wav (about 635 MB an hour while QEMU runs)"
  $AudioDev = "wav,id=audio0,path=$Wav"
} else {
  Write-Error "-Audio must be off, speakers, file or file:<path.wav> (got '$Audio')."
  exit 3
}
if ($env:AZ_AUDIODEV) { $AudioDev = $env:AZ_AUDIODEV }
if (Get-Process qemu-system-aarch64 -ErrorAction SilentlyContinue) {
  Write-Error 'qemu-system-aarch64 is already running: ONE emulator (ports 2222/4445/4446).'
  exit 4
}
# AZ_INITRD / AZ_MNT_IMG / AZ_USB_FILE: boot another image, settings disk or USB stick
# (as in az-boot-m1.sh; tools/az-emu-test.py boots a no-mods copy of the image).
$Initrd = if ($env:AZ_INITRD) { $env:AZ_INITRD } else { Join-Path $Build 'initramfs-patched.cpio.gz' }
$MntImg = if ($env:AZ_MNT_IMG) { $env:AZ_MNT_IMG } else { Join-Path $Build 'az-mnt.img' }
$Usb = if ($env:AZ_USB_FILE) { $env:AZ_USB_FILE } else { Join-Path $Build 'usb.img' }
foreach ($f in (Join-Path $Build 'Image'), $Initrd, $MntImg, $Usb) {
  if (-not (Test-Path $f)) {
    Write-Error "missing $f. Pull your deck's system (tools/az-emu-pull.py) and run ./build.sh first (docs/windows.md)."
    exit 2
  }
}
# The deck storage needs AZ_MNT_MIN_GB (default 14), as in az-boot-m1.sh: an older,
# smaller image is enlarged (marked sparse first, so it costs only what the guest
# writes) and the guest grows its reserve partition into the new space (patch 32).
$MinGB = if ($env:AZ_MNT_MIN_GB) { [int]$env:AZ_MNT_MIN_GB } else { 14 }
if ($MinGB -gt 0 -and (Test-Path -PathType Leaf $MntImg) -and (Get-Item $MntImg).Length -lt ([int64]$MinGB * 1GB)) {
  Write-Host "enlarging $MntImg to $MinGB GB so the deck storage fits the deck program's caches"
  & fsutil sparse setflag $MntImg | Out-Null
  $fs = [IO.File]::Open($MntImg, 'Open', 'ReadWrite')
  try { $fs.SetLength([int64]$MinGB * 1GB) } finally { $fs.Close() }
}
$Extra = if ($env:AZ_TESTMODE) { ' subucom_testmode' } else { '' }
Set-Content -Path $Serial -Value $null

function Q([string]$s) { '"' + $s + '"' }   # every arg quoted: paths may hold spaces
$qargs = @(
  '-machine', 'virt', '-accel', ('tcg,thread=multi' + $(if ($Tcg) { ",$Tcg" } else { '' })), '-cpu', $Cpu, '-smp', "$Smp", '-m', "${Mem}B",
  '-kernel', (Q "$Build\Image"),
  '-initrd', (Q $Initrd),
  '-append', (Q "root=/dev/ram0 rdinit=/init loglevel=7 nowatchdog console=ttyAMA0,115200 systemd.journald.forward_to_console=1 snd_dummy.index=1 dummy.numdummies=0 nokaslr$Extra"),
  '-display', $Display,
  # IPv4 only (slirp): connect to 127.0.0.1:2222, never "localhost", which Windows
  # resolves to ::1 first: the connect succeeds and the banner exchange is reset.
  '-netdev', 'user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22',
  '-device', 'virtio-net-device,netdev=net0',
  '-serial', (Q "file:$Serial"),
  '-qmp', 'tcp:127.0.0.1:4445,server=on,wait=off',
  '-qmp', 'tcp:127.0.0.1:4446,server=on,wait=off',
  '-device', 'virtio-rng-device',
  '-audiodev', (Q $AudioDev),
  '-device', 'virtio-sound-device,audiodev=audio0,streams=2',
  '-device', 'virtio-gpu-device,id=gpu0,xres=1280,yres=800,edid=off,max_outputs=1',
  '-device', 'virtio-tablet-pci,id=tablet0',
  '-drive', (Q "file=$MntImg,if=none,id=mnt0,format=raw,cache=writeback"),
  '-device', 'virtio-blk-device,drive=mnt0',
  '-drive', (Q "file=$Usb,if=none,id=usbdrv,format=raw,cache=writeback"),
  '-device', 'usb-ehci,id=ehci',
  '-device', 'usb-storage,bus=ehci.0,drive=usbdrv',
  '-device', 'virtio-serial-device,max_ports=8'
)
foreach ($p in 'main', 'sub', 'ctrl', 'gpio') {
  $qargs += '-chardev', (Q "socket,id=v$p,path=$Sock\$p.sock,server=on,wait=off")
  $qargs += '-device', "virtserialport,chardev=v$p,name=cdj3k.$p"
}
# Launch through ShellExecute (no -Redirect* on Start-Process), via a hidden cmd that
# owns the stderr redirect. Any -RedirectStandard* makes .NET CreateProcess with
# bInheritHandles, so QEMU inherits EVERY inheritable handle of this PowerShell,
# including a caller's stdout pipe, and a caller that captures our output
# (az-preview) then waits for QEMU to EXIT. Measured with ping -n 15 as the child:
# -RedirectStandardError alone 14.4 s, both redirected 14.4 s, ShellExecute via cmd 0.2 s.
$QemuErr = Join-Path $Sock 'qemu.err'
$line = (Q $Qemu) + ' ' + ($qargs -join ' ') + ' 2>' + (Q $QemuErr)
$shell = Start-Process -FilePath $env:ComSpec -ArgumentList '/s', '/c', ('"' + $line + '"') `
  -WindowStyle Hidden -PassThru
$qpid = $null
for ($i = 0; $i -lt 50 -and -not $qpid; $i++) {
  Start-Sleep -Milliseconds 100
  $qpid = (Get-CimInstance Win32_Process -Filter "ParentProcessId=$($shell.Id) AND Name='qemu-system-aarch64.exe'").ProcessId
}
if (-not $qpid) {
  Write-Error "QEMU did not start (cmd pid $($shell.Id)); see $QemuErr"
  exit 5
}
Remove-Item -ErrorAction SilentlyContinue (Join-Path $Sock 'mods-status.txt'), (Join-Path $Sock 'health-status.txt')
if (-not $NoMods -and $env:AZ_EMU_NOMODS -ne '1') {
  # Same ShellExecute-through-cmd launch as QEMU above, for the same reason: a caller
  # capturing our output must not wait on this child. Log: mods-install.log. When it
  # cannot confirm the installed set is the newest, AZ-EMU.app shows its "AZ-MODS:"
  # line in the title bar (from mods-status.txt).
  $ModsLog = Join-Path $Sock 'mods-install.log'
  $env:AZ_EMU_STICK = if ($Usb -like '\\.\*') { 'real' } else { 'emulated' }   # inherited by the child
  # python, else the py launcher (docs/windows.md accepts either)
  $Py = if (Get-Command python -ErrorAction SilentlyContinue) { 'python' }
        elseif (Get-Command py -ErrorAction SilentlyContinue) { 'py -3' }
        else { $null }
  if ($Py) {
    $mline = "$Py " + (Q (Join-Path $Repo 'tools\az-mods-fetch.py')) + ' --wait 300 >' + (Q $ModsLog) + ' 2>&1'
    Start-Process -FilePath $env:ComSpec -ArgumentList '/s', '/c', ('"' + $mline + '"') -WindowStyle Hidden | Out-Null
  } else {
    Write-Warning 'no python or py on PATH: the AZ-MODS check is skipped (tools/az-mods-fetch.py)'
  }
}
# The guest's health once the deck program runs (tools/az-emu-health.py): an image older
# than this checkout, a full deck storage, a missing OneLibrary key flag. AZ-EMU.app
# shows what it finds in its title bar (from health-status.txt). TCG boots slowly, so
# it waits longer than on a Mac.
$HPy = if (Get-Command python -ErrorAction SilentlyContinue) { 'python' }
       elseif (Get-Command py -ErrorAction SilentlyContinue) { 'py -3' }
       else { $null }
if ($HPy) {
  # A watcher from a previous AZ-EMU holds health.log open; stop it before ours starts
  # (it also exits on its own once its QEMU is gone).
  $HPidFile = Join-Path $Sock 'health.pid'
  if (Test-Path $HPidFile) {
    # Only a pid that is still that watcher is stopped; a stale pid file is just
    # removed, since Windows may have handed its number to an unrelated program.
    try {
      $hpid = [int](Get-Content $HPidFile -ErrorAction Stop)
      $hproc = Get-CimInstance Win32_Process -Filter "ProcessId=$hpid" -ErrorAction Stop
      if ($hproc -and $hproc.CommandLine -and $hproc.CommandLine.Contains('az-emu-health.py')) {
        Stop-Process -Id $hpid -Force -ErrorAction SilentlyContinue
      }
    } catch {}
    Remove-Item $HPidFile -Force -ErrorAction SilentlyContinue
  }
  $hline = "$HPy " + (Q (Join-Path $Repo 'tools\az-emu-health.py')) + ' --wait 900 --watch 10 >' + (Q (Join-Path $Sock 'health.log')) + ' 2>&1'
  Start-Process -FilePath $env:ComSpec -ArgumentList '/s', '/c', ('"' + $hline + '"') -WindowStyle Hidden | Out-Null
}
"qemu pid $qpid  smp=$Smp  cpu=$Cpu  tcg=$Tcg  serial=$Serial  sockets=$Sock  QMP 4445(app)/4446(automation)  ssh: python tools\az-drive.py or tools\az-ssh"
