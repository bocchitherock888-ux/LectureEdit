import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { demoAdapter } from './demo';
import type { AppState, DomainCommand, LectureAdapter, RuntimeInfo } from './types';
import { canExportNativePdf, PDF_PLATFORM_NOTICE } from './platform';
import { applyDelta, type StateDelta, type SyncedState } from './sync';

const isTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

// The library can be tens of megabytes; each call fetches only the lectures
// that changed since the revision held here.
let held: SyncedState | null = null;
async function syncState(command?: DomainCommand): Promise<AppState> {
  const delta = await invoke<StateDelta>('sync', { command: command ?? null, epoch: held?.epoch ?? '', since: held?.revision ?? 0 });
  let next = applyDelta(held, delta);
  // Never resend the command: it has already run. Just fetch everything.
  if (!next) next = applyDelta(null, await invoke<StateDelta>('sync', { command: null, epoch: '', since: 0 }));
  if (!next) throw new Error('无法读取课程数据，请重启“随堂”。');
  held = next;
  return next.state;
}

const nativeAdapter: LectureAdapter = {
  mode: 'native',
  dispatch: (command: DomainCommand) => syncState(command),
  runtimeInfo: () => invoke<RuntimeInfo>('runtime_info'),
  openHelpLink: (id) => invoke<void>('open_help_link', { id }),
  startRecording: (sessionId, source) => syncState({ type: 'startRecording', commandId: crypto.randomUUID(), sessionId, source }),
  pauseRecording: (sessionId) => syncState({ type: 'pauseRecording', commandId: crypto.randomUUID(), sessionId }),
  stopRecording: (sessionId) => syncState({ type: 'stopRecording', commandId: crypto.randomUUID(), sessionId }),
  retryInference: (sessionId) => syncState({ type: 'retryInference', commandId: crypto.randomUUID(), sessionId }),
  importAudio: async (sessionId) => {
    const path = await open({ multiple: false, filters: [{ name: 'WAV 音频', extensions: ['wav'] }] });
    if (!path) return null;
    return syncState({ type: 'importAudio', commandId: crypto.randomUUID(), sessionId, path });
  },
  importPackage: async () => {
    const path = await open({ multiple: false, filters: [{ name: '随堂课程包', extensions: ['lecture', 'zip'] }] });
    if (!path) return null;
    return syncState({ type: 'importPackage', commandId: crypto.randomUUID(), path });
  },
  exportSession: async (sessionId, format) => {
    if (format === 'pdf') {
      const runtime = await invoke<RuntimeInfo>('runtime_info');
      if (!canExportNativePdf(runtime.platform)) throw new Error(PDF_PLATFORM_NOTICE);
      const state = await syncState({ type: 'snapshot', sessionId });
      const session = state.sessions.find((item) => item.id === sessionId);
      if (!session) throw new Error('找不到要导出的课程。');
      const fileName = `${session.title.replace(/[\\/:*?"<>|]/g, '-').trim() || '随堂课程'}.pdf`;
      const path = await save({ defaultPath: fileName, filters: [{ name: '精排 PDF', extensions: ['pdf'] }] });
      if (!path) return;
      const latestState = await syncState({ type: 'snapshot', sessionId });
      const latestSession = latestState.sessions.find((item) => item.id === sessionId);
      if (!latestSession) throw new Error('找不到要导出的课程。');
      const prepared = await (await import('./pdfExport')).preparePdfExport(latestSession);
      try { await invoke('export_pdf', { path, pageCount: prepared.pageCount, pageRects: prepared.pageRects }); }
      finally { prepared.cleanup(); }
      return;
    }
    const extension = format === 'markdown' ? 'md' : format === 'lecture' ? 'lecture' : format;
    const path = await save({ filters: [{ name: `${format.toUpperCase()} 导出`, extensions: [extension] }] });
    if (!path) return;
    await invoke('dispatch', { command: { type: 'export', commandId: crypto.randomUUID(), sessionId, format, path } });
  },
  audioData: (sessionId, runId, startSample, endSample) => invoke<string>('audio_data', { sessionId, runId, startSample, endSample }),
  translateText: (text, targetLanguage) => invoke<{ text: string }>('translate_text', { request: { text, targetLanguage, consent: true } }),
  recognizeFormula: (imageData) => invoke<{ latex: string }>('recognize_formula', { request: { imageData, consent: true } }),
  pickFile: async (filters) => {
    const result = await open({ multiple: false, filters });
    return typeof result === 'string' ? result : null;
  },
};

export const adapter = isTauri() ? nativeAdapter : demoAdapter;
