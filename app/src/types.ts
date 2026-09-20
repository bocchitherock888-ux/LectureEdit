export type RecordingState = 'idle' | 'starting' | 'recording' | 'paused' | 'stopping' | 'stopped' | 'error';
export type InferenceState = 'unloaded' | 'loading' | 'ready' | 'running' | 'catching_up' | 'paused' | 'error';

export interface Settings {
  engine: string;
  executable: string;
  modelPath: string;
  mmprojPath: string;
  language: string;
  customVocabulary?: string[];
  cloudConsent?: boolean;
  autoPolish?: boolean;
  theme?: 'system' | 'light' | 'dark';
}

export interface Segment {
  id: string;
  runId: string;
  startSample: number;
  endSample: number;
  machineText: string;
  machineRevision: number;
  final: boolean;
  workerEpoch: number;
  userSeq: number;
  displayText: string;
  pendingMachine: string | null;
  corrected: boolean;
  history: unknown[];
  historyIndex: number;
}

export interface Draft {
  id: string;
  segmentId: string;
  baseMachineText: string;
  baseMachineRevision: number;
  snapshotText: string;
  expectedUserSeq: number;
  text: string;
  revision: number;
  state: string;
}

export type NoteKind = 'note' | 'example' | 'formula' | 'image';
export interface Note {
  id: string;
  segmentId: string;
  runId: string;
  sample: number;
  kind: NoteKind;
  text: string;
  sourceLabel: string | null;
  imageData: string | null;
}

export interface Run {
  id: string;
  engine?: string;
  source: 'microphone' | 'system' | string;
  startedAt: number;
  endedAt: number | null;
  samples: number;
  offsetMs: number;
  state: string;
}

export interface Gap { runId: string; startSample: number; endSample: number; reason: string }

export interface Project {
  id: string;
  title: string;
  createdAt: number;
  customVocabulary?: string[];
}

export interface Session {
  id: string;
  projectId: string | null;
  title: string;
  createdAt: number;
  mode: 'live' | 'demo';
  recordingState: RecordingState;
  inferenceState: InferenceState;
  segments: Segment[];
  drafts: Draft[];
  notes: Note[];
  runs: Run[];
  gaps: Gap[];
  operationSeq: number;
  error: string | null;
  customVocabulary?: string[];
}

export interface AppState {
  schemaVersion: 1;
  projects: Project[];
  sessions: Session[];
  selectedSessionId: string | null;
  settings: Settings;
  workerEpoch: number;
}

export interface RuntimeInfo {
  platform?: string;
  modelInstalled?: boolean;
  localModelInstalled?: boolean;
  modelReady?: boolean;
  modelState?: 'unloaded' | 'loading' | 'ready' | 'error';
  modelError?: string | null;
  cloudKeyConfigured?: boolean;
  deepseekKeyConfigured?: boolean;
  cloudProcessing?: boolean;
  cloudSessionId?: string | null;
  modelDownload?: {
    phase: 'idle' | 'downloading' | 'verifying' | 'complete' | 'error';
    downloadedBytes: number;
    totalBytes: number;
    fileName: string | null;
    error: string | null;
  };
  defaultExecutable?: string;
  defaultModelPath?: string;
  defaultMmprojPath?: string;
  systemAudioAvailable?: boolean;
  defaults?: Partial<Settings>;
}

export type DomainCommand = { type: string; commandId?: string; [key: string]: unknown };

export interface LectureAdapter {
  readonly mode: 'native' | 'demo';
  dispatch(command: DomainCommand): Promise<AppState>;
  runtimeInfo(): Promise<RuntimeInfo>;
  startRecording(sessionId: string, source: 'microphone' | 'system'): Promise<AppState>;
  pauseRecording(sessionId: string): Promise<AppState>;
  stopRecording(sessionId: string): Promise<AppState>;
  retryInference(sessionId: string): Promise<AppState>;
  importAudio(sessionId: string): Promise<AppState | null>;
  importPackage(): Promise<AppState | null>;
  exportSession(sessionId: string, format: 'markdown' | 'html' | 'pdf' | 'wav' | 'lecture'): Promise<void>;
  audioData(sessionId: string, runId: string, startSample: number, endSample: number): Promise<string>;
  translateText(text: string, targetLanguage: 'zh' | 'en'): Promise<{ text: string }>;
  recognizeFormula(imageData: string): Promise<{ latex: string }>;
  pickFile(filters?: { name: string; extensions: string[] }[]): Promise<string | null>;
}

export const commandId = () => globalThis.crypto?.randomUUID?.() ?? `cmd-${Date.now()}-${Math.random().toString(16).slice(2)}`;
