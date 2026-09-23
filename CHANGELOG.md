# Changelog

## 0.1.1 — 2026-09-23

### Windows

- Fix local Qwen model failing to start ("本地识别程序提前退出"): llama.cpp now uses the shared MSVC runtime, bundled beside it
- Work with non-ASCII user folders (e.g. `C:\Users\张伟`)
- Helper processes now exit with the app and are stopped before upgrade or uninstall
- System audio keeps timing through silence and follows default output device changes
- Use the Windows certificate store for downloads and cloud connections (corporate proxies)
- CI now loads the packaged model and transcribes real speech before release

### All platforms

- Show why the local model stopped, with a log file path
- Skip re-hashing verified model files on every start; turn off llama.cpp's 8 GB prompt cache
- Fewer re-renders while idle; smoother long transcripts
- Live transcript: words no longer lose their spaces while fading in; lighter reveal animation
- Correction panel: now a floating card that fits the sentence's height; the transcript slides aside instead of reflowing; the sentence flies into the editor and lands without a jump
- Many fixes to error display, shortcuts, dark mode and transcription edge cases

Upgrading: run the new installer over the old one. Courses, recordings and the downloaded model are kept.

## 0.1.0 — 2026-09-20

First public test release.

### Features

- Record a lecture from the microphone or system audio, and keep PCM on disk
- Transcribe locally with Qwen3-ASR (llama.cpp); optional Soniox cloud engine
- Revise the current sentence while recording continues
- Insert notes, formulas and images at audio anchors
- Search across all lectures
- Export Markdown, HTML, WAV and `.lecture` course packages
- Pretty-print PDF on macOS; Windows can export HTML and print to PDF from Edge

### Platforms

- macOS 13+ on Apple Silicon
- Windows 11 x64 (Intel/AMD)

Unsigned test builds. Model weights download separately on first use.
