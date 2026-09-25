import type { AppState, DomainCommand, Draft, LectureAdapter, Note, Project, Segment, Session } from './types';

const now = Date.now();
const demoSegments: Segment[] = [
  { id: 'seg-1', runId: 'run-demo', startSample: 0, endSample: 187200, machineText: 'Let us begin with an everyday question: why does the same cup of coffee cost more at an airport than on campus? Economics studies how people make choices when time and resources are limited, and how those choices affect one another.', machineRevision: 1, final: true, workerEpoch: 1, userSeq: 0, displayText: 'Let us begin with an everyday question: why does the same cup of coffee cost more at an airport than on campus? Economics studies how people make choices when time and resources are limited, and how those choices affect one another.', pendingMachine: null, corrected: false, history: [], historyIndex: 0 },
  { id: 'seg-2', runId: 'run-demo', startSample: 187200, endSample: 403200, machineText: 'Opportunity cost is the money we pay for a choice. For example, taking a course on Saturday involves tuition and the activities we give up.', machineRevision: 2, final: true, workerEpoch: 1, userSeq: 1, displayText: 'Opportunity cost is the value of the best alternative we give up when making a choice. Taking a course on Saturday involves tuition, but it also means giving up time that could have been spent working or resting.', pendingMachine: null, corrected: true, history: [], historyIndex: 1 },
  { id: 'seg-3', runId: 'run-demo', startSample: 403200, endSample: 648000, machineText: 'Now consider supply and demand. A change in price affects willingness to buy, while income, preferences and substitutes also shape consumer choices. We need to distinguish price changes from changes in other conditions.', machineRevision: 1, final: true, workerEpoch: 1, userSeq: 1, displayText: 'Now consider supply and demand. A change in price affects the quantity demanded, while income, preferences and substitutes also shape consumer choices. We need to distinguish a movement along the demand curve from a shift of the curve.', pendingMachine: 'A change in the price of coffee causes a movement along its demand curve. Changes in income or preferences can shift the entire curve.', corrected: true, history: [], historyIndex: 1 },
  { id: 'seg-4', runId: 'run-demo', startSample: 648000, endSample: 801600, machineText: 'Returning to our coffee shop, we can now examine how rent, foot traffic and the availability of alternatives influence the price customers are willing to pay.', machineRevision: 1, final: false, workerEpoch: 1, userSeq: 0, displayText: 'Returning to our coffee shop, we can now examine how rent, foot traffic and the availability of alternatives influence the price customers are willing to pay.', pendingMachine: null, corrected: false, history: [], historyIndex: 0 },
];

const demoProjects: Project[] = [
  { id: 'project-economics', title: 'Economics', createdAt: now - 24 * 60_000, customVocabulary: [] },
];

const demoSession = (): Session => ({
  id: 'demo-economics', projectId: 'project-economics', title: 'Economics 101 · Markets & Choices', createdAt: now - 22 * 60_000,
  mode: 'demo', recordingState: 'idle', inferenceState: 'ready', segments: demoSegments,
  drafts: [],
  notes: [
    { id: 'note-1', segmentId: 'seg-2', runId: 'run-demo', sample: 187200, kind: 'note', text: 'Opportunity cost includes the value of time. Compare attending a Saturday class with taking a paid shift.', sourceLabel: 'My notes', imageData: null },
    { id: 'note-2', segmentId: 'seg-3', runId: 'run-demo', sample: 403200, kind: 'example', text: 'A price cut at the same coffee shop changes the quantity demanded. A new shop nearby changes the available substitutes.', sourceLabel: 'Lecture example', imageData: null },
  ],
  runs: [{ id: 'run-demo', source: 'microphone', startedAt: now - 22 * 60_000, endedAt: now - 8 * 60_000, samples: 801600, offsetMs: 0, state: 'stopped' }], gaps: [], operationSeq: 12, error: null,
  customVocabulary: [],
});

let state: AppState = {
  schemaVersion: 1, projects: demoProjects, sessions: [demoSession()], selectedSessionId: 'demo-economics', workerEpoch: 1,
  settings: { engine: 'qwen', executable: '', modelPath: '', mmprojPath: '', language: 'en', customVocabulary: [], autoPolish: false, theme: 'system' },
};
const storageKey = 'lectureedit-preview-v4-en';
try {
  const saved = typeof sessionStorage === 'undefined' ? null : sessionStorage.getItem(storageKey);
  if (saved) {
    const parsed = JSON.parse(saved) as AppState;
    if (parsed.schemaVersion === 1 && Array.isArray(parsed.sessions) && parsed.sessions.every((session) => session.mode === 'demo')) {
      const projects = Array.isArray(parsed.projects) ? parsed.projects.map((project) => ({ ...project, customVocabulary: project.customVocabulary ?? [] })) : [];
      const projectIds = new Set(projects.map((project) => project.id));
      const sessions = parsed.sessions.map((session) => ({ ...session, projectId: session.projectId ?? null, customVocabulary: session.customVocabulary ?? [] }));
      if (sessions.some((session) => session.projectId !== null && !projectIds.has(session.projectId))) throw new Error('INVALID_PROJECT_REFERENCE');
      state = { ...parsed, projects, sessions, settings: { ...parsed.settings, customVocabulary: parsed.settings.customVocabulary ?? [], autoPolish: parsed.settings.autoPolish ?? false, theme: parsed.settings.theme ?? 'system' } };
      for (const session of state.sessions) {
        if (session.recordingState === 'recording') {
          session.recordingState = 'stopped'; session.inferenceState = 'ready';
          for (const run of session.runs.filter((item) => item.state === 'recording')) { run.state = 'stopped'; run.endedAt = Date.now(); }
          for (const segment of session.segments) segment.final = true;
        }
      }
    }
  }
} catch { /* A fresh preview works when browser storage is unavailable. */ }
const persistPreview = () => {
  try { if (typeof sessionStorage !== 'undefined') sessionStorage.setItem(storageKey, JSON.stringify(state)); } catch { /* The preview retains its current in-memory state. */ }
};
const lectureLines = [
  'A useful starting point is to specify which conditions remain constant before asking how a change in one factor affects the outcome.',
  'When comparing two coffee shops, consider location, service, product quality and the time customers are willing to spend.',
  'When a decision affects other people, we should ask whether those effects are reflected in the costs faced by the decision maker.',
  'The assumptions of a model help us focus on a specific question. They also tell us where its conclusions are likely to apply.',
  'For each decision, ask which alternatives are available, what each option offers, and what must be given up in return.',
];
const previewCursors = new Map<string, { line: number; words: number; segmentId: string | null }>();
const clone = <T,>(value: T): T => structuredClone(value);
const selected = () => state.sessions.find((s) => s.id === state.selectedSessionId);
const titleBytes = (value: string) => new TextEncoder().encode(value).byteLength;
const validTitle = (value: unknown) => {
  if (typeof value !== 'string') throw new Error('请输入课程名称。');
  const title = value.trim();
  if (!title || titleBytes(title) > 4096) throw new Error('课程名称必须为 1–4096 字节。');
  return title;
};
const projectId = (value: unknown, optional: boolean) => {
  if ((value === undefined && optional) || value === null) return null;
  if (typeof value !== 'string' || !state.projects.some((project) => project.id === value)) throw new Error('找不到这个项目。');
  return value;
};
const uniqueId = (prefix: string) => {
  let id: string;
  do { id = `${prefix}-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(16).slice(2)}`}`; }
  while (state.projects.some((project) => project.id === id) || state.sessions.some((session) => session.id === id));
  return id;
};
const mutateSession = (id: string, fn: (session: Session) => void) => {
  const session = state.sessions.find((item) => item.id === id);
  if (!session) throw new Error('找不到这门课程。');
  fn(session); session.operationSeq += 1;
};

async function dispatch(command: DomainCommand): Promise<AppState> {
  await new Promise((resolve) => setTimeout(resolve, command.type === 'snapshot' ? 20 : 90));
  const id = String(command.sessionId ?? state.selectedSessionId ?? '');
  switch (command.type) {
    case 'snapshot': break;
    case 'createProject': state.projects.push({ id: uniqueId('project'), title: validTitle(command.title), createdAt: Date.now(), customVocabulary: [] }); break;
    case 'renameProject': {
      const project = state.projects.find((item) => item.id === command.projectId);
      if (!project) throw new Error('找不到这个项目。');
      project.title = validTitle(command.title); break;
    }
    case 'moveSession': {
      if (!Object.prototype.hasOwnProperty.call(command, 'projectId')) throw new Error('请选择项目。');
      const destination = projectId(command.projectId, false);
      mutateSession(id, (session) => { session.projectId = destination; }); break;
    }
    case 'createSession': {
      const session: Session = { id: uniqueId('session'), projectId: projectId(command.projectId, true), title: validTitle(command.title), createdAt: Date.now(), mode: (command.mode === 'live' ? 'live' : 'demo'), recordingState: 'idle', inferenceState: 'ready', segments: [], drafts: [], notes: [], runs: [], gaps: [], operationSeq: 1, error: null, customVocabulary: [] };
      state.sessions.unshift(session); state.selectedSessionId = session.id; break;
    }
    case 'selectSession': state.selectedSessionId = id; break;
    case 'renameSession': mutateSession(id, (s) => { s.title = String(command.title).trim() || s.title; }); break;
    case 'beginEdit': mutateSession(id, (s) => {
      const segment = s.segments.find((x) => x.id === command.segmentId); if (!segment) throw new Error('找不到这段转写。');
      let draft = s.drafts.find((x) => x.segmentId === segment.id);
      if (!draft) { draft = { id: `draft-${Date.now()}`, segmentId: segment.id, baseMachineText: segment.machineText, baseMachineRevision: segment.machineRevision, snapshotText: segment.displayText, expectedUserSeq: segment.userSeq, text: segment.displayText, revision: 0, state: 'drafting' }; s.drafts.push(draft); }
    }); break;
    case 'saveDraft': mutateSession(id, (s) => { const d = s.drafts.find((x) => x.id === command.draftId); if (!d) throw new Error('草稿已经关闭。'); const rev = Number(command.revision); if (rev >= d.revision) { d.text = String(command.text); d.revision = rev; d.state = 'saved'; } }); break;
    case 'commitEdit': mutateSession(id, (s) => { const d = s.drafts.find((x) => x.id === command.draftId); if (!d) throw new Error('草稿已经关闭。'); const seg = s.segments.find((x) => x.id === d.segmentId); if (!seg) throw new Error('转写块已经不可用。'); if (Number(command.expectedUserSeq) !== seg.userSeq) throw new Error('此段已有新的人工修订。草稿仍已保留，请重新打开比较。'); const keep = Math.max(0, seg.historyIndex + 1); seg.history = [...seg.history.slice(0, keep), { text: String(command.text) }]; seg.historyIndex = seg.history.length - 1; seg.displayText = String(command.text); seg.corrected = seg.displayText !== seg.machineText; seg.userSeq += 1; s.drafts = s.drafts.filter((x) => x.id !== d.id); }); break;
    case 'closeDraft': break;
    case 'discardDraft': mutateSession(id, (s) => { s.drafts = s.drafts.filter((x) => x.id !== command.draftId); }); break;
    case 'resolve': mutateSession(id, (s) => { const seg = s.segments.find((x) => x.id === command.segmentId); if (!seg) return; if (command.action === 'machine' && seg.pendingMachine) { seg.displayText = seg.pendingMachine; seg.corrected = false; } else if (command.text) seg.displayText = String(command.text); seg.pendingMachine = null; seg.userSeq += 1; }); break;
    case 'undo': mutateSession(id, (s) => { const seg = s.segments.find((x) => x.id === command.segmentId); if (!seg || Number(command.expectedUserSeq) !== seg.userSeq || seg.historyIndex < 0) throw new Error('当前没有可撤销的修订。'); seg.historyIndex -= 1; const item = seg.history[seg.historyIndex] as { text?: string } | undefined; seg.displayText = item?.text ?? seg.machineText; seg.corrected = Boolean(item); seg.userSeq += 1; }); break;
    case 'redo': mutateSession(id, (s) => { const seg = s.segments.find((x) => x.id === command.segmentId); if (!seg || Number(command.expectedUserSeq) !== seg.userSeq || seg.historyIndex >= seg.history.length - 1) throw new Error('当前没有可重做的修订。'); seg.historyIndex += 1; const item = seg.history[seg.historyIndex] as { text?: string } | undefined; seg.displayText = item?.text ?? seg.machineText; seg.corrected = Boolean(item); seg.userSeq += 1; }); break;
    case 'addNote': mutateSession(id, (s) => { const seg = s.segments.find((x) => x.id === command.segmentId); if (!seg) throw new Error('请先选择一个转写块。'); const note: Note = { id: `note-${Date.now()}`, segmentId: seg.id, runId: seg.runId, sample: seg.startSample, kind: command.kind as Note['kind'], text: String(command.text ?? ''), sourceLabel: command.sourceLabel ? String(command.sourceLabel) : null, imageData: command.imageData ? String(command.imageData) : null }; s.notes.push(note); }); break;
    case 'removeNote': mutateSession(id, (s) => { s.notes = s.notes.filter((x) => x.id !== command.noteId); }); break;
    case 'settings': {
      state.settings = { ...state.settings, ...(command.settings as object) };
      if (typeof command.projectId === 'string' && Array.isArray(command.projectVocabulary)) {
        const project = state.projects.find((item) => item.id === command.projectId);
        if (project) project.customVocabulary = command.projectVocabulary.map(String);
      }
      if (typeof command.sessionId === 'string' && Array.isArray(command.sessionVocabulary)) {
        const session = state.sessions.find((item) => item.id === command.sessionId);
        if (session) session.customVocabulary = command.sessionVocabulary.map(String);
      }
      break;
    }
    case 'demoTick': {
      const s = selected();
      if (s?.recordingState === 'recording') {
        const run = s.runs.find((item) => item.state === 'recording');
        if (!run) break;
        run.samples += 11200;
        const cursor = previewCursors.get(run.id) ?? { line: 0, words: 0, segmentId: null };
        const words = lectureLines[cursor.line % lectureLines.length].split(' ');
        let segment = s.segments.find((item) => item.id === cursor.segmentId);
        if (!segment) {
          segment = { id: `preview-${run.id}-${cursor.line}`, runId: run.id, startSample: Math.max(0, run.samples - 11200), endSample: run.samples, machineText: '', displayText: '', machineRevision: 0, final: false, workerEpoch: state.workerEpoch, userSeq: 0, pendingMachine: null, corrected: false, history: [], historyIndex: 0 };
          s.segments.push(segment); cursor.segmentId = segment.id;
        }
        cursor.words = Math.min(words.length, cursor.words + 5);
        const text = words.slice(0, cursor.words).join(' ');
        segment.machineText = text; segment.machineRevision += 1; segment.endSample = run.samples;
        if (segment.corrected) { if (segment.displayText !== text) segment.pendingMachine = text; }
        else segment.displayText = text;
        segment.final = cursor.words === words.length;
        if (segment.final) { cursor.line += 1; cursor.words = 0; cursor.segmentId = null; }
        previewCursors.set(run.id, cursor); s.operationSeq += 1;
      }
      break;
    }
    case 'importSession': { const imported = { ...(command.session as Session), projectId: null, customVocabulary: (command.session as Session).customVocabulary ?? [] }; state.sessions.unshift(imported); state.selectedSessionId = imported.id; break; }
    default: throw new Error(`演示适配器不支持命令：${command.type}`);
  }
  if (command.type !== 'snapshot') persistPreview();
  return clone(state);
}

export const demoAdapter: LectureAdapter = {
  mode: 'demo', dispatch,
  openHelpLink: async (_id, url) => { window.open(url, '_blank', 'noopener'); },
  runtimeInfo: async () => ({ platform: '浏览器', modelInstalled: false, modelReady: false, modelState: 'unloaded', modelError: null }),
  startRecording: async (sessionId) => { mutateSession(sessionId, (s) => { if (s.recordingState === 'recording') return; s.recordingState = 'recording'; s.inferenceState = 'running'; s.runs.push({ id: `run-${Date.now()}`, source: 'microphone', startedAt: Date.now(), endedAt: null, samples: 0, offsetMs: s.runs.reduce((sum, run) => sum + run.samples / 16, 0), state: 'recording' }); }); persistPreview(); return clone(state); },
  pauseRecording: async (sessionId) => { mutateSession(sessionId, (s) => { s.recordingState = 'paused'; for (const run of s.runs.filter((item) => item.state === 'recording')) { run.endedAt = Date.now(); run.state = 'closed'; for (const segment of s.segments.filter((item) => item.runId === run.id)) segment.final = true; } }); persistPreview(); return clone(state); },
  stopRecording: async (sessionId) => { mutateSession(sessionId, (s) => { s.recordingState = 'stopped'; s.inferenceState = 'ready'; for (const run of s.runs.filter((item) => item.state === 'recording')) { run.endedAt = Date.now(); run.state = 'stopped'; for (const segment of s.segments.filter((item) => item.runId === run.id)) segment.final = true; } }); persistPreview(); return clone(state); },
  retryInference: async (sessionId) => { mutateSession(sessionId, (s) => { s.inferenceState = 'ready'; s.error = null; }); return clone(state); },
  importAudio: async () => { throw new Error('浏览器演示只展示交互。请在“随堂”桌面应用中导入 WAV。'); },
  importPackage: async () => { throw new Error('浏览器演示只展示交互。请在“随堂”桌面应用中导入课程包。'); },
  exportSession: async () => { throw new Error('浏览器演示不会写入文件。请在“随堂”桌面应用中导出。'); },
  audioData: async () => { throw new Error('这份确定性演示数据没有附带音频。'); },
  translateText: async () => { throw new Error('请在桌面应用中配置 DeepSeek 并翻译。'); },
  recognizeFormula: async () => { throw new Error('请在桌面应用中配置 DeepSeek 并识别公式。'); },
  pickFile: async () => null,
};
