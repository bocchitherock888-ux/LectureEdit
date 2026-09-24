# Changelog

## 0.1.4 — 2026-09-24

- The app is now called 随堂 (Suitang). Courses, recordings, models and saved API keys stay where they were. On Windows the new installer removes the old LectureEdit install; on Mac, delete the old LectureEdit app after installing
- Each lecture is stored on its own, so a transcript update during a long lecture rewrites only that lecture instead of the whole library (about 9× faster on a 34 MB library). The library is converted once on first open, after a copy of the old file is saved next to it (`*.before-0.1.4`); 0.1.3 and earlier cannot open the converted library
- A lecture that cannot be read is set aside and the rest open; a damaged library file is kept aside and a fresh one opens. The app says what was set aside
- If the microphone disconnects mid-lecture, recording switches to another input and continues; the gap is marked
- A short guide (8 steps) opens the first time the app starts with an empty library. It can be skipped, and reopened from Settings
- DeepSeek auto-polish failures show as a small mark next to the toolbar instead of a notice; hover it for the reason
- Mac: the sentence toolbar (修订, 笔记, 翻译, 回放) did nothing when clicked, and live text could be drawn blank or over the next sentence; both fixed. Windows did not have these problems
- ⌘E / Ctrl+E edits the sentence you clicked, or the newest one if none is selected
- The window fetches only what changed instead of the whole library every 700 ms, and only the live paragraph redraws
- Sentences split across recognition chunks are merged when polished; note cards, the sentence toolbar and live follow scrolling were refined
- Equations and PDF export load on demand; the app starts with a smaller bundle

## 0.1.3 — 2026-09-23

- The live transcript now rolls upward smoothly instead of jumping between the middle and the bottom of the window. New paragraphs used a placeholder height until drawn, so the page grew and shrank; the newest paragraphs are now always fully laid out, and following only moves forward with a short glide
- Dragging the scrollbar while the transcript is gliding stops following, as before
- Windows CI reuses the compiled llama.cpp runtime and Rust dependencies between releases; the packaged model still transcribes real speech on every build

## 0.1.2 — 2026-09-23

- Vocabulary words no longer appear in the transcript by themselves (e.g. "Tocqueville. Tocqueville." after the speaker stops) or replace a different word. When a local transcript contains a vocabulary word, a second pass without the vocabulary has to hear something similar in the same place.
- Segments with less than 200 ms of speech (a click or a breath) are no longer sent to the model
- macOS: the microphone recorded silence in 0.1.1 because the build lacked the audio-input entitlement; fixed, and the release script now checks for it
- Warn when the microphone delivers only silence for 3 seconds, with where to grant access on macOS or Windows

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
