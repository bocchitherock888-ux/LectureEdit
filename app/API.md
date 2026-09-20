# Integration contract

The React client uses Tauri commands. The browser preview implements the same adapter with local demonstration data. Recording, inference, playback and editing remain independently controlled.

## Tauri commands

- `dispatch({command}) -> State`: domain and native operations. Persistent mutations use `type` and `commandId`; command arguments use camelCase.
- `runtime_info() -> RuntimeInfo`: model readiness and active cloud work. It contains no API key.
- `audio_data({sessionId, runId, startSample, endSample}) -> string`: a base64 WAV data URL for at most 60 seconds of saved audio.
- `export_pdf({path, pageCount, pageRects}) -> ()`: asynchronously capture prepared A4 WebView pages as vector PDF data and save one validated `.pdf` on macOS. `pageCount` is the Paged.js result; `pageRects` contains the matching WebView document rectangles.

The UI polls snapshots every 700 ms and confirms a write after dispatch resolves. Model preparation runs at startup and after settings changes. Local recording requires `modelReady === true`; the backend also enforces readiness.

## State

The TypeScript declarations in `src/types.ts` mirror Rust's serde camelCase structs in `src-tauri/src/domain.rs`.

| Object | Fields |
| --- | --- |
| State | `schemaVersion: 1`, `projects`, `sessions`, `selectedSessionId`, `settings`, `workerEpoch` |
| Settings | `engine: qwen / whisper / soniox`, `executable`, `modelPath`, `mmprojPath`, `language: auto / en / zh`, `customVocabulary: string[]`, `cloudConsent`, `autoPolish`, `theme: system / light / dark` |
| Project | `id`, `title`, `createdAt` |
| Session | `id`, `title`, `createdAt`, `projectId: string \| null`, `mode`, `recordingState`, `inferenceState`, `segments`, `drafts`, `notes`, `runs`, `gaps`, `operationSeq`, `error` |
| Segment | `id`, `runId`, `startSample`, `endSample`, `machineText`, `machineRevision`, `final`, `workerEpoch`, `userSeq`, `displayText`, `pendingMachine`, `corrected`, `history`, `historyIndex` |
| Draft | `id`, `segmentId`, `baseMachineText`, `baseMachineRevision`, `snapshotText`, `expectedUserSeq`, `text`, `revision`, `state` |
| Note | `id`, `segmentId`, `runId`, `sample`, `kind`, `text`, `sourceLabel`, `imageData` |
| Run | `id`, `engine`, `source`, `startedAt`, `endedAt`, `samples`, `offsetMs`, `state` |
| Gap | `runId`, `startSample`, `endSample`, `reason` |

Samples refer to stored 16 kHz mono PCM. A run pins its engine when created; older run records default to Qwen. Inference states include `unloaded`, `loading`, `ready`, `running`, `catching_up`, `paused` and `error`.

Legacy snapshots without `projects` or session `projectId` deserialize as an empty project list with ungrouped sessions.

`RuntimeInfo` exposes `platform`, `modelInstalled`, `localModelInstalled`, `modelReady`, `modelState`, `modelError`, `modelDownload`, `cloudKeyConfigured`, `cloudProcessing`, `cloudSessionId`, `defaultExecutable`, `defaultModelPath`, `defaultMmprojPath`, `systemAudioAvailable`, `dataDirectory` and `defaults`. For Soniox, readiness means credentials and consent are configured; the connection is established when work starts. `cloudSessionId` identifies the active task even if the selected settings change. `localModelInstalled` describes local files independently of the selected engine.

`modelDownload` contains `phase: idle / downloading / verifying / complete / error`, `downloadedBytes`, `totalBytes`, `fileName` and `error`. Progress comes from streamed bytes, including verified files and resumable partials. Installation preserves partial files on interrupted transfers, validates Range responses, and checks file size and SHA-256 before promoting files. Model weights remain outside the application bundle. Downloading blocks engine/settings changes and conflicting recording or cloud work.

## Domain operations

- `snapshot`
- `createProject {title}`, `renameProject {projectId, title}`, `moveSession {sessionId, projectId: string | null}`
- `createSession {title, mode, projectId?: string | null}`, `selectSession {sessionId}`, `renameSession {sessionId, title}`
- `beginEdit {sessionId, segmentId}`, `saveDraft {sessionId, draftId, text, revision}`, `commitEdit {sessionId, draftId, text, expectedUserSeq}`
- `closeDraft {sessionId, draftId}`, `discardDraft {sessionId, draftId}`
- `resolve {sessionId, segmentId, expectedUserSeq, expectedMachineRevision, action: keep / machine, text?}`
- `undo / redo {sessionId, segmentId, expectedUserSeq}`
- `addNote {sessionId, segmentId, kind: note / example / formula / image, text, sourceLabel?, imageData?}`, `removeNote {sessionId, noteId}`
- `settings {settings}`, `demoTick {sessionId}`, `importSession {session}`

Worker-only operations are `machine {sessionId, segmentId, runId, startSample, endSample, text, revision, workerEpoch, final, allowFinalRangeShrink?}` and atomic `machineSegments {sessionId, runId, workerEpoch, segments[]}`. The latter replaces a final recognition window with deterministic natural-sentence segments while preserving edits and drafts attached to the first ID. The domain rejects stale epochs and preserves acknowledged human changes with conservative merge and explicit pending proposals. Range shrink is accepted only when a Soniox final token boundary replaces a longer provisional processed-audio estimate.

Project and session titles are trimmed, must contain text and may occupy at most 4096 UTF-8 bytes. Project IDs are safe and unique. A non-null session membership must reference an existing project. Commands with invalid or dangling references are rejected transactionally.

## Native operations

- `prepareModel`: request model preparation and return immediately.
- `installModel {engine: qwen}`: download or resume hash-verified model files; preserve language and cloud consent, select the installed local engine, and request preparation. Poll `runtime_info` for progress while this command is pending.
- `startRecording {sessionId, source: microphone / system}` and `stopRecording {sessionId}`.
- `importAudio {sessionId, path}`: import WAV audio and queue recognition with the selected engine.
- `retryInference {sessionId}`: actively restart pending work for the selected engine.
- `export {sessionId, format: markdown / html / wav / lecture, path}` and `importPackage {path}`.
- `configureSoniox {apiKey}`: replace the process-memory credential; an empty string clears it. This command bypasses persisted receipts and state.
- `cancelCloud {sessionId}`: request cancellation immediately, preserve audio and queued jobs, and set inference to paused. Local recording continues. Wait until `cloudProcessing` is false before retrying.

Cloud audio is sent only after an explicit start/import/retry with Soniox selected and consent enabled. Startup never resumes uploads. Cloud finalization continues after capture stops; pausing a cloud task preserves unfinished work. Imported audio is paced at real time to meet the streaming endpoint contract.

Imported `.lecture` packages enter the library ungrouped. Exported packages serialize `projectId: null`, so their contents remain independent of the source library's project organization.

## PDF export

After the native save dialog returns, the frontend requests a fresh snapshot and builds escaped A4 pages with Paged.js. It switches the WebView to a finite capture layout, waits for fonts and images, then passes `pageCount` and one document-coordinate rectangle per `.pagedjs_page` to `export_pdf`:

```text
export_pdf({
  path,
  pageCount,
  pageRects: [{x, y, width, height}, ...]
}) -> ()
```

`pageCount` must be 1–500 and must equal `pageRects.length`. Rectangle values must be finite, `x` and `y` must be nonnegative, and every rectangle must be within 3 CSS pixels of A4 at 96 dpi: approximately 793.70 × 1122.52 CSS pixels. The total captured WebKit PDF data is limited to 256 MiB. The destination must end in `.pdf` and its parent directory must already exist.

On macOS, the backend uses asynchronous `WKWebView.createPDF` calls with a `WKPDFConfiguration` rectangle and an opaque background to capture each prepared page separately. Each capture has a 60-second timeout and must parse as exactly one PDF page. Core Graphics places the captured vector pages into a single A4 PDF. PDFKit then verifies the expected page count, 595.28 × 841.89 point page bounds within 0.5 point, and readable non-whitespace text on every page. The file is written and synchronized through an atomic sibling temporary file, reopened and validated again, then renamed into place. Unsupported platforms return an explicit error.

All user text is escaped before it enters the print document. Formula rendering uses KaTeX with `trust: false`; embedded images accept only PNG, JPEG and WebP data URLs.

## Local search

Library search runs entirely in the client. It matches project titles, session titles, corrected transcript `segment.displayText`, note text and note source labels. Query terms are case-insensitive and use AND semantics. Results are ordered by session creation time, newest first.

`SearchResult` contains `id`, `sessionId`, `segmentId`, `noteId`, `title`, `projectTitle`, `createdAt`, `text`, `startMs`, `audioAnchor: {runId, startSample, endSample} | null` and `kind: title | transcript | note`. Transcript search anchors use the segment's run and sample range. The reading view derives sentence-level playback anchors for any persisted long segment by distributing its sample range according to spoken-text weight; this leaves stored text, corrections and note relationships unchanged. Note anchors use the note's independent `runId` and `sample`. Display time is absolute within the session and includes `Run.offsetMs`.

Search navigation flushes pending draft saves, selects the target session, reveals its project and scrolls to the matching note or segment. Navigation generations and playback request tokens prevent stale playback from a previous session.

## Durability and concurrency

`Store::open`, `snapshot`, `dispatch` and `mutate` operate on a clone, validate it, commit a SQLite transaction, then replace in-memory state. Store locks are held only for state operations. SQLite uses WAL and FULL synchronization. Native side effects have separate started/done/failed receipts. API keys never enter snapshots, receipts or exports.

Final inference jobs and PCM are independently durable. Machine output carries an epoch and monotonic revision. Recording publishes its saved sample boundary before exposing inference work. Audio gaps remain explicit records; recovery retains original files and quarantines malformed job records.

## DeepSeek text and image tools

- `dispatch({command:{type:"configureDeepSeek",apiKey}})`: update the memory-only credential; an empty key clears it. No persisted command receipt is created.
- `translate_text({request:{text,targetLanguage:"zh"|"en",consent:true}}) -> {text}`: translate at most 12,000 characters.
- `recognize_formula({request:{imageData,consent:true}}) -> {latex}`: transcribe formulas from a PNG/JPEG/WebP data URL, with a 5 MiB decoded limit and file-signature validation.
- `runtime_info` exposes only `deepseekKeyConfigured`, never the credential.

When `settings.autoPolish` is enabled, a final Qwen job or Soniox final update schedules `PolishRequest {text, consent:true}` on a background thread. `final` means the speech engine has committed the current utterance boundary; it does not mean the whole lecture or visual paragraph is complete. The request uses the current projected `displayText`, including earlier human edits. `polishSegment` commits only when `final`, `machineRevision`, `userSeq` and `sourceText` still match. It appends a `Correction`, leaves `machineText` untouched and becomes part of ordinary undo/redo history. Network, model and stale-version errors are ignored by the capture path.

The fixed endpoint is the official DeepSeek Chat Completions API, using `deepseek-flash` (V4.1 Flash as of 2026-09-19), non-thinking mode and JSON output. Requests have a 60-second timeout and a 256 KiB response cap. Translation/formula results are transient until the user saves through existing `addNote` operations. Audio capture and inference remain independent. The UI captures the selected text before opening a preview, requires an explicit action to send, and rejects stale responses after dismissal, image changes or course switches. Model output is rendered as text or untrusted KaTeX.
