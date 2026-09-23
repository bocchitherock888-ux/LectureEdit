<div align="right"><a href="README.md">中文</a></div>

<img src="app/public/app-icon.png" width="72" height="72" alt="LectureEdit icon">

# LectureEdit

[![最新版本](https://img.shields.io/github/v/release/bocchitherock888-ux/LectureEdit?label=release&color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit/releases/latest)
[![MIT License](https://img.shields.io/github/license/bocchitherock888-ux/LectureEdit?color=356d73)](LICENSE)
[![下载量](https://img.shields.io/github/downloads/bocchitherock888-ux/LectureEdit/total?label=downloads&color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit/releases)
[![构建](https://github.com/bocchitherock888-ux/LectureEdit/actions/workflows/verify.yml/badge.svg)](https://github.com/bocchitherock888-ux/LectureEdit/actions/workflows/verify.yml)
![macOS 13+](https://img.shields.io/badge/macOS-13%2B%20Apple%20Silicon-555555?logo=apple)
![Windows x64](https://img.shields.io/badge/Windows-x64-555555?logo=windows)
[![主要语言](https://img.shields.io/github/languages/top/bocchitherock888-ux/LectureEdit?color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit)
[![语言数](https://img.shields.io/github/languages/count/bocchitherock888-ux/LectureEdit?label=languages&color=356d73)](https://github.com/bocchitherock888-ux/LectureEdit)

**Record, edit the transcript and add notes as you listen.**

LectureEdit is a local-first desktop application for lecture recording and transcription. Open a sentence in the left-hand editor while new transcript text continues on the right. Save your correction to put it back in place, with the associated recording and notes preserved.

When a lecturer's name is transcribed incorrectly, correct it while you remember the spelling. Add an equation or paste a slide image beside the relevant passage. Later, search for the topic, replay the recording and continue working on your notes.

[Downloads and releases](https://github.com/bocchitherock888-ux/LectureEdit/releases) · [Get started](#get-started) · [Report an issue](https://github.com/bocchitherock888-ux/LectureEdit/issues)

<!-- Add a screenshot or recording of the actual desktop app here, showing editing on the left and continuing transcription on the right. -->

## Downloads and platforms

The current version is **0.1.3 beta**.

| Platform | Status |
|---|---|
| macOS 13 or later, Apple Silicon | Beta builds provided as DMG and ZIP packages. |
| Windows x64, 64-bit Intel / AMD PCs | Unsigned NSIS beta installer provided. Live recording and transcription on a real PC are still pending. |

For Mac, download the appropriate package from [Releases](https://github.com/bocchitherock888-ux/LectureEdit/releases), move LectureEdit to Applications and open it. The current Mac build is ad-hoc signed and awaits Apple notarisation. After checking the download source, follow the system prompt under System Settings → Privacy & Security to allow it to open.

On Windows, run `LectureEdit_*_windows-x64_multi_setup.exe`. To upgrade, run the new installer over the old version: it removes the old version and installs to the same folder. Courses, recordings and the downloaded model are kept unless you tick "Delete the application data".

The local model downloads separately on first use. Packaged-app users complete setup inside LectureEdit.

The application interface is currently in Simplified Chinese. The instructions below include the relevant interface labels.

## Get started

1. Open **模型与服务** (Models and services), select Qwen3-ASR local recognition and choose **下载模型** (Download model). The current Qwen3-ASR-0.6B Q8 model and its audio projection file total approximately 1.02 GB. Downloads show progress, can resume after interruption and are checked for integrity when complete.
2. Wait for **本地模型已就绪** (Local model ready). Create a course group and a recording for this lecture. For an English-language class, set recognition to **英语** (English). Chinese and automatic language detection are also available.
3. Choose an audio source and start recording. Use the microphone for an in-person class or system audio for material playing on your computer. System-audio capture requires operating-system permissions and further hardware validation. Existing WAV recordings can be imported through **更多操作** (More actions).
4. To correct the latest sentence, choose **修订当前句** (Edit current sentence) or press `⌘E`. Edit and save on the left while transcription continues on the right. The Windows shortcut is `Ctrl+E`.

Obtain the appropriate permission before recording, and check the input source and transcription with a short sample first.

## Edit during the lecture

### Work on a sentence while transcription continues

The sentence editor saves your draft automatically. Earlier passages have their own **取出编辑** (Open for editing) action. Audio capture and subsequent recognition continue while you edit. Saving puts the corrected text back into the transcript and resumes live following.

Human corrections and the original machine transcript are stored separately. When a later recognition result conflicts with an edit, LectureEdit presents it for review. Keep the current text, accept the recognition result or merge them manually. Corrections have undo and redo history.

Scrolling back preserves your reading position. Choose **回到实时** (Back to live) to follow new text again. Separate **暂停录音** and **继续录音** controls pause and resume the recording itself.

### Add equations, images and examples where they belong

Select a sentence and choose **添加资料** (Add material) to attach a note, example, LaTeX equation or image. Choose an image file or paste an image. Equations are rendered as mathematical notation.

Materials stay attached to the relevant transcript passage and retain their audio-time association. Add an example while it is being discussed, then revisit it with the surrounding explanation.

### Set vocabulary for your courses

Use **常用词与标准拼写** (Vocabulary and preferred spellings) under Models and services to add names, technical terms and abbreviations, one per line. You can also specify an explicit spelling replacement:

```text
Amartya Sen
Kahneman
con yard → Cournot
```

Vocabulary can apply to all courses, the current course or the current lecture. The three levels are combined. For competing replacements, lecture-specific entries take priority, followed by course entries and then global entries.

Preferred terms are supplied as recognition hints. Entries with an arrow apply spelling replacements when recognised text is committed. You maintain the vocabulary and can adjust it as course content changes.

## Review and organise your notes

After recording, choose **编辑全文** (Edit full transcript) to work on the complete document. Saving preserves its audio associations, attached materials and correction history.

Organise subjects into course groups. Each lecture has its own name and date, and can be renamed or moved to another group. Choose **搜索所有课堂** (Search all lectures), or press `⌘K` / `Ctrl+K`, to search course names, transcripts and notes. Search uses the corrected transcript text.

Results can take you to the original passage or play the associated recording. In the transcript, **回放这句** (Replay this sentence) plays the selected sentence. Sentence-level playback within Qwen segments and older long segments uses estimated positions for convenient review; the exact range depends on transcription segmentation.

Choose a light, dark or system-matched appearance, and collapse the course sidebar to make more room for reading.

## Local recognition and optional cloud tools

Qwen3-ASR recognises speech on your computer by default. Once the model is ready, local transcription, manual editing, notes, search and export work offline. For a fully local workflow, select local recognition and keep cloud assistance switched off.

Optional cloud tools use API keys that you provide. The following integrations are experimental, with live-service acceptance testing still pending:

| Tool | Purpose | Content sent to the provider |
|---|---|---|
| Soniox stt-rt-v5 | Cloud-based live speech recognition | Audio from recordings, imports or retries you actively start, together with configured vocabulary hints. |
| DeepSeek light cleanup | Tidy obvious sentence breaks, isolated fillers, punctuation and capitalisation | Confirmed transcript paragraphs, sent automatically while enabled. |
| DeepSeek selected-text translation | Translate selected text into Simplified Chinese or English, with the option to save it as a note | The text selected when you invoke translation. |
| DeepSeek image-to-equation recognition | Generate editable LaTeX from an image, then review and save it as an equation | The image you choose to recognise. |

Light cleanup retains the original recognition output and undoable correction history. Review translations and recognised equations before saving, particularly important terminology and mathematical expressions.

With Soniox selected, the original audio is still saved locally. **暂停云端转写** (Pause cloud transcription) stops subsequent audio uploads while local recording continues. Pending work can be retried after the connection returns. WAV imports sent to Soniox are streamed at normal playback speed, so processing time depends on recording length.

Provider charges go to your own account. API keys use the operating system's credential store: Keychain on macOS and Credential Manager on Windows. Course packages and document exports exclude keys.

## Export, back up and continue editing

Choose a format from **导出** (Export):

| Format | Use |
|---|---|
| Markdown | Transcript and text materials, with equations retained as LaTeX text for further editing. |
| Reading HTML | A browser-readable text version of the transcript and text materials. |
| Typeset PDF | Pages for reading, sharing and printing, including equations, images and page numbers. Currently available on macOS. |
| WAV audio | The lecture recording. |
| `.lecture` package | The individual lecture's audio, transcript, corrections and attached materials, ready to import back into LectureEdit. |

**Choose a `.lecture` package for a complete backup of an individual lecture.** Image attachments are preserved in the package. Markdown and HTML exports currently focus on text. Import a package to continue editing and assign it to a course group.

Lecture data is stored in the local application data directory. Packages include recordings and attached materials, so review their contents before sharing and keep separate backups of important work.

## Beta status

Start with a short recording to try the complete workflow. Windows packaging and hardware testing, system-audio capture, live cloud services and 120-minute continuous recording acceptance tests are pending. Recognition speed and quality depend on your computer, audio quality and chosen service.

File import currently accepts WAV. The current Mac packages target Apple Silicon. Check each release for its installation files and validation status.

## Development and feedback

LectureEdit uses Tauri 2, Rust, React, TypeScript and SQLite. Local Qwen3-ASR inference runs through llama.cpp, and KaTeX renders equations.

See [app/README.md](app/README.md) for the development environment and commands, and [WINDOWS_BUILD.md](WINDOWS_BUILD.md) for Windows x64 builds. These developer guides are currently in Chinese.

Created by **Tipram** (醉步羊). Original code is [MIT](LICENSE). Third-party components and model weights are described in [NOTICE.md](NOTICE.md).

When [reporting an issue](https://github.com/bocchitherock888-ux/LectureEdit/issues), include the application version, operating system and processor, audio source, recognition engine, steps to reproduce and what happened. Use a short, authorised recording with personal information removed when an audio sample is needed.
====================================================================
