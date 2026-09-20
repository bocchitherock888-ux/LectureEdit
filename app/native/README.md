# LectureEdit native system-audio helpers

These helpers are standalone child processes. They begin capture only when launched, write raw 16,000 Hz mono signed 16-bit little-endian PCM to standard output, and reserve standard error for lifecycle messages. `READY` means capture started. When a bounded queue overflows, the helper inserts silence to keep sample offsets stable and emits `GAP:<start-sample>:<end-sample>`. `ERROR:<stage>:<message>` reports a failure. Send a newline on standard input for a clean stop; successful shutdown emits `STOPPED`.

## macOS

`macos-system-audio/SystemAudioCapture.swift` uses ScreenCaptureKit on macOS 13 or newer. It excludes audio from the helper process and filters the parent LectureEdit process from captured applications when ScreenCaptureKit exposes it. Build on macOS with:

```sh
./macos-system-audio/build.sh
```

The executable is written to `bin/lectureedit-capture`. The application build can copy it to `resource/native/lectureedit-capture`. Building does not request Screen Recording permission or start capture. Launching the executable can show the system permission prompt.

## Windows

`windows-loopback` uses shared-mode WASAPI loopback capture for the default console render endpoint. Build it in a Windows x64 Developer PowerShell with:

```powershell
rustup target add x86_64-pc-windows-msvc
./windows-loopback/build.ps1
```

CI should run `cargo fmt --check`, `cargo clippy --target x86_64-pc-windows-msvc -- -D warnings`, and `cargo build --release --locked --target x86_64-pc-windows-msvc` from `native/windows-loopback`. The Windows runtime capture test is `not_run` on macOS.

The PowerShell build copies the executable to `bin/lectureedit-loopback.exe`; packaging can copy that file to `resource/native/lectureedit-loopback.exe`.
