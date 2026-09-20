/** Design contract. Runtime validation and Rust code generation are production work. */
export type Id = string;
export type SampleIndex = number; // Runtime: non-negative safe integer, within one capture run.
export type UserSeq = number; // Monotonic, including undo and explicit resolution.
export interface AudioRange {
  captureRunId: Id;
  startSample: SampleIndex;
  endSample: SampleIndex; // Exclusive; >= startSample.
  sampleRate: 16000;
}
export type RecordingState = 'idle'|'starting'|'recording'|'paused'|'stopping'|'stopped'|'error';
export type InferenceState = 'unloaded'|'loading'|'ready'|'running'|'catching_up'|'error';
export interface BackendCapabilities {
  engineId: string;
  modelId: string;
  audioInputMode: 'chunked'|'native_streaming';
  tokenStreaming: boolean;
  timestampGranularity: 'none'|'segment'|'word';
  contextHints: boolean;
  languageSelection: boolean;
  cancellation: 'cooperative'|'kill_worker'|'none';
}
export interface MachineSnapshot {
  segmentId: Id;
  workerEpoch: number;
  revision: number; // Normalised by host; monotonic per segment across worker restarts.
  text: string;
  final: boolean;
  audio: AudioRange;
  confidence?: number; // Only present if the backend provides a meaningful value.
}
export interface ProtectedEdit {
  intentId: Id;
  baseMachineRevision: number;
  startGrapheme: number;
  endGrapheme: number;
  replacement: string;
  // Remains protected even if a later machine hypothesis matches the replacement.
}
export interface CorrectionRevision {
  id: Id;
  segmentId: Id;
  parentId: Id|null;
  userSeq: UserSeq;
  baseMachineRevision: number;
  baseMachineText: string;
  userText: string;
  protectedEdits: ProtectedEdit[];
  createdAt: string;
}
export interface DisplayProjection {
  text: string;
  status: 'machine'|'corrected'|'pending_review';
  pendingMachine?: {text: string; revision: number; reason: string};
  alignmentState: 'segment_only'|'valid_word_alignment'|'stale_word_alignment';
}
export interface EditSession {
  id: Id;
  segmentId: Id;
  baseMachineRevision: number;
  baseMachineText: string;
  snapshotDisplayText: string;
  expectedUserSeq: UserSeq;
  draftText: string;
  draftRevision: number;
}
export interface NoteAnchor {
  sessionId: Id;
  segmentId: Id;
  captureRunId: Id;
  sample: SampleIndex;
  side: 'before'|'after';
  rank: string; // Fractional ordering value, stable within the anchor.
}
export type RichTextNode = {
  type: string; text?: string; attrs?: Record<string, unknown>; content?: RichTextNode[];
}; // Actual runtime must allowlist node types and attrs.
export type NoteContent =
  | {kind: 'note'|'example'; title?: string; document: RichTextNode; sourceLabel?: string}
  | {kind: 'formula'; latex: string; displayMode: boolean; caption?: string}
  | {kind: 'image'; assetId: Id; alt: string; caption?: string};
export interface NoteBlock {id: Id; anchor: NoteAnchor; revision: number; content: NoteContent; tombstone: boolean;}
export type Command =
  | {type: 'begin_edit'; commandId: Id; segmentId: Id}
  | {type: 'save_draft'; commandId: Id; editorId: Id; draftRevision: number; text: string}
  | {type: 'commit_edit'; commandId: Id; editorId: Id; expectedUserSeq: UserSeq; text: string}
  | {type: 'resolve_candidate'; commandId: Id; segmentId: Id; expectedUserSeq: UserSeq;
     expectedMachineRevision: number; action: 'keep_human'|'accept_machine'|'manual'; text?: string}
  | {type: 'add_note'; commandId: Id; anchor: NoteAnchor; content: NoteContent}
  | {type: 'stop_recording'; commandId: Id; sessionId: Id; captureRunId: Id};
export interface DomainError {code: string; message: string; retryable: boolean; draftPreserved?: boolean;}
export interface EventEnvelope<T> {
  schemaVersion: 1;
  eventId: Id;
  sessionId: Id;
  operationSeq: number;
  committedAt: string;
  payload: T;
}
export type DomainPayload =
  | {type: 'segment_updated'; segmentId: Id; projection: DisplayProjection; machineRevision: number}
  | {type: 'correction_committed'; commandId: Id; segmentId: Id; userSeq: UserSeq; projection: DisplayProjection}
  | {type: 'note_added'; note: NoteBlock}
  | {type: 'recording_changed'; state: RecordingState; captureRunId: Id}
  | {type: 'gap_recorded'; audio: AudioRange; reason: string};
export interface BackendJob {
  jobId: Id; sessionId: Id; segmentId: Id; workerEpoch: number;
  kind: 'partial'|'final'|'reprocess'; audio: AudioRange;
}
export interface BackendAdapter {
  capabilities(): BackendCapabilities;
  load(modelManifestId: Id): Promise<void>;
  transcribeSnapshot(job: BackendJob): AsyncIterable<MachineSnapshot>;
  cancel(jobId: Id): Promise<void>;
  unload(): Promise<void>;
}
