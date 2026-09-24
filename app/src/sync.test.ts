import { describe, expect, it } from 'vitest';
import { applyDelta, shareSession, type StateDelta } from './sync';
import type { AppState, Session } from './types';

const meta: Omit<AppState, 'sessions'> = { schemaVersion: 1, projects: [], selectedSessionId: 'a', settings: {} as AppState['settings'], workerEpoch: 1 };
const session = (id: string, title = id) => ({ id, title, segments: [], notes: [], runs: [] }) as unknown as Session;
const full = (sessions: Session[], revision = 3): StateDelta => ({ epoch: 'e1', revision, full: true, meta: { ...meta }, order: sessions.map((s) => s.id), sessions });

describe('applyDelta', () => {
  it('builds state from a full reply in server order', () => {
    const synced = applyDelta(null, full([session('b'), session('a')]))!;
    expect(synced.state.sessions.map((s) => s.id)).toEqual(['b', 'a']);
    expect(synced.state.selectedSessionId).toBe('a');
    expect(synced.revision).toBe(3);
  });

  it('replaces only changed lectures and keeps the rest by identity', () => {
    const held = applyDelta(null, full([session('a'), session('b')]))!;
    const next = applyDelta(held, { epoch: 'e1', revision: 4, full: false, order: ['a', 'b'], sessions: [session('b', 'B2')] })!;
    expect(next.state.sessions[0]).toBe(held.state.sessions[0]);
    expect(next.state.sessions[1].title).toBe('B2');
    expect(next.state.settings).toBe(held.state.settings);
  });

  it('adds, reorders and drops lectures from the order list', () => {
    const held = applyDelta(null, full([session('a'), session('b')]))!;
    const next = applyDelta(held, { epoch: 'e1', revision: 5, full: false, meta: { ...meta, selectedSessionId: 'c' }, order: ['c', 'a'], sessions: [session('c')] })!;
    expect(next.state.sessions.map((s) => s.id)).toEqual(['c', 'a']);
    expect(next.state.selectedSessionId).toBe('c');
  });

  it('ignores a reply older than what it holds', () => {
    const held = applyDelta(null, full([session('a')], 9))!;
    expect(applyDelta(held, { epoch: 'e1', revision: 8, full: false, order: ['a'], sessions: [session('a', 'old')] })).toBe(held);
  });

  it('asks for a full resync when it cannot fill a lecture', () => {
    const held = applyDelta(null, full([session('a')]))!;
    expect(applyDelta(held, { epoch: 'e1', revision: 4, full: false, order: ['a', 'x'], sessions: [] })).toBeNull();
    expect(applyDelta(held, { epoch: 'other', revision: 4, full: false, order: ['a'], sessions: [] })).toBeNull();
  });

  it('reuses unchanged sentences and notes inside a changed lecture', () => {
    const seg = (id: string, displayText: string) => ({ id, displayText, history: [{ text: displayText }] });
    const before = { id: 's', title: 'L', segments: [seg('1', 'a'), seg('2', 'b')], notes: [{ id: 'n', text: 'x' }], runs: [{ id: 'r', samples: 10 }] } as unknown as Session;
    const after = JSON.parse(JSON.stringify({ ...before, segments: [seg('1', 'a'), seg('2', 'b!'), seg('3', 'c')], runs: [{ id: 'r', samples: 20 }] })) as Session;
    const shared = shareSession(before, after);
    expect(shared.segments[0]).toBe(before.segments[0]);
    expect(shared.segments[1]).not.toBe(before.segments[1]);
    expect(shared.segments[1].displayText).toBe('b!');
    expect(shared.segments[2].id).toBe('3');
    expect(shared.notes).toBe(before.notes);
    expect(shared.runs).not.toBe(before.runs);
    const identical = JSON.parse(JSON.stringify(before)) as Session;
    expect(shareSession(before, identical)).toBe(before);
  });
});
