import { describe, expect, it } from 'vitest';
import { searchExcerpt, searchLibrary } from './search';
import type { Session } from './types';

const classroom = (id: string, title: string, text: string, createdAt: number): Session => ({
  id, title, createdAt, projectId: 'econ', mode: 'live', recordingState: 'stopped', inferenceState: 'ready', drafts: [], gaps: [], operationSeq: 1, error: null,
  runs: [{ id: 'run-2', offsetMs: 90_000, samples: 320_000, startedAt: createdAt, endedAt: createdAt + 20_000, source: 'import', state: 'stopped' }],
  segments: [{ id: 'segment', runId: 'run-2', startSample: 32_000, endSample: 120_000, displayText: text, machineText: 'obsolete transcription', machineRevision: 1, final: true, workerEpoch: 1, corrected: true, userSeq: 1, pendingMachine: null, history: [], historyIndex: 0 }], notes: [],
});
const projects = [{ id: 'econ', title: 'Economics 101', createdAt: 1 }];

describe('searchLibrary', () => {
  it('finds corrected content across classrooms, newest first, with absolute audio offsets', () => {
    const sessions = [classroom('one', 'Week 1', 'Opportunity cost of a lecture', 1), classroom('two', 'Week 2', 'Opportunity cost of working', 2)];
    const results = searchLibrary(sessions, projects, 'OPPORTUNITY COST');
    expect(results.map(result => result.sessionId)).toEqual(['two', 'one']);
    expect(results[0]).toMatchObject({ segmentId: 'segment', startMs: 92_000, projectTitle: 'Economics 101' });
    expect(searchLibrary(sessions, projects, 'obsolete')).toEqual([]);
  });
  it('finds notes and courses without mutating sessions or requiring transcript matches', () => {
    const session = classroom('one', 'Week 1', 'A price change', 1);
    session.notes.push({ id: 'note', segmentId: 'segment', runId: 'run-2', sample: 32_000, kind: 'note', text: '需求曲线', sourceLabel: 'Reading', imageData: null });
    expect(searchLibrary([session], projects, '需求')[0]).toMatchObject({ kind: 'note', noteId: 'note', startMs: 92_000 });
    expect(searchLibrary([session], projects, 'Reading')[0].text).toContain('Reading');
    expect(searchLibrary([session], projects, 'Economics')[0].kind).toBe('title');
    expect(searchLibrary([session], projects, '   ')).toEqual([]);
    expect(session.title).toBe('Week 1');
  });
  it('shows the match in a long excerpt and supports literal punctuation', () => {
    const text = `${'intro '.repeat(100)}demand (price) ${'closing '.repeat(80)}`;
    expect(searchExcerpt(text, '(price)', 120)).toContain('(price)');
    expect(searchLibrary([classroom('one', 'Week 1', text, 1)], projects, '(price)')).toHaveLength(1);
  });
  it('preserves a note anchor later than the start of its transcript segment', () => {
    const session = classroom('one', 'Week 1', 'A price change', 1);
    session.notes.push({ id: 'later-note', segmentId: 'segment', runId: 'run-2', sample: 96_000, kind: 'note', text: 'Elasticity', sourceLabel: null, imageData: null });
    const result = searchLibrary([session], projects, 'elasticity')[0];
    expect(result.startMs).toBe(96_000);
    expect(result.audioAnchor).toEqual({ runId: 'run-2', startSample: 96_000, endSample: 120_000 });
  });
});
