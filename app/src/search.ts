import type { Project, Session } from './types';

export interface SearchResult {
  id: string;
  sessionId: string;
  segmentId: string | null;
  noteId: string | null;
  title: string;
  projectTitle: string;
  createdAt: number;
  text: string;
  startMs: number | null;
  audioAnchor: { runId: string; startSample: number; endSample: number } | null;
  kind: 'transcript' | 'note' | 'title';
}

export function searchLibrary(sessions: Session[], projects: Project[], query: string): SearchResult[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  if (!terms.length) return [];
  const matches = (text: string) => terms.every(term => text.toLocaleLowerCase().includes(term));
  const results: SearchResult[] = [];
  const ordered = [...sessions].sort((a, b) => b.createdAt - a.createdAt);
  for (const session of ordered) {
    const projectTitle = projects.find(project => project.id === session.projectId)?.title ?? '';
    const base = { sessionId: session.id, title: session.title, projectTitle, createdAt: session.createdAt };
    if (matches(`${projectTitle} ${session.title}`)) {
      results.push({ ...base, id: `${session.id}:title`, kind: 'title', segmentId: null, noteId: null, text: session.title, startMs: null, audioAnchor: null });
    }
    for (const segment of session.segments) {
      const run = session.runs.find(item => item.id === segment.runId);
      const startMs = (run?.offsetMs ?? 0) + segment.startSample / 16;
      if (matches(segment.displayText)) {
        results.push({ ...base, id: `${session.id}:${segment.id}`, kind: 'transcript', segmentId: segment.id, noteId: null, text: segment.displayText, startMs, audioAnchor: { runId: segment.runId, startSample: segment.startSample, endSample: segment.endSample } });
      }
      for (const note of session.notes.filter(item => item.segmentId === segment.id)) {
        const noteText = [note.text, note.sourceLabel].filter(Boolean).join(' · ');
        if (matches(noteText)) {
          const noteRun = session.runs.find(item => item.id === note.runId);
          results.push({ ...base, id: `${session.id}:${note.id}`, kind: 'note', segmentId: segment.id, noteId: note.id, text: noteText, startMs: (noteRun?.offsetMs ?? 0) + note.sample / 16, audioAnchor: { runId: note.runId, startSample: note.sample, endSample: Math.max(note.sample, segment.endSample) } });
        }
      }
    }
  }
  return results;
}

export function searchExcerpt(text: string, query: string, limit = 210): string {
  if (text.length <= limit) return text;
  const terms = query.trim().toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  const indexes = terms.map(term => text.toLocaleLowerCase().indexOf(term)).filter(index => index >= 0);
  const index = indexes.length ? Math.min(...indexes) : 0;
  const start = Math.max(0, Math.min(text.length - limit, index - 65));
  return `${start ? '…' : ''}${text.slice(start, start + limit)}${start + limit < text.length ? '…' : ''}`;
}
