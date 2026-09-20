import { beforeEach, describe, expect, it, vi } from 'vitest';

describe('preview interaction contract', () => {
  beforeEach(() => vi.resetModules());
  it('keeps a recoverable draft across live transcript updates', async () => {
    const { demoAdapter } = await import('./demo');
    const sessionId = (await demoAdapter.dispatch({ type: 'snapshot' })).selectedSessionId!;
    await demoAdapter.startRecording(sessionId, 'microphone');
    const tick = await demoAdapter.dispatch({ type: 'demoTick', sessionId });
    const segment = tick.sessions.find((s) => s.id === sessionId)!.segments.at(-1)!;
    const edit = await demoAdapter.dispatch({ type: 'beginEdit', sessionId, segmentId: segment.id });
    const draft = edit.sessions.find((s) => s.id === sessionId)!.drafts.at(-1)!;
    expect(['committed', 'discarded']).not.toContain(draft.state);
    await demoAdapter.dispatch({ type: 'saveDraft', sessionId, draftId: draft.id, revision: 1, text: 'My correction while listening.' });
    const result = await demoAdapter.dispatch({ type: 'demoTick', sessionId });
    expect(result.sessions.find((s) => s.id === sessionId)!.drafts.at(-1)!.text).toBe('My correction while listening.');
  });
  it('retains a human correction when the same live utterance grows', async () => {
    const { demoAdapter } = await import('./demo');
    const sessionId = (await demoAdapter.dispatch({ type: 'snapshot' })).selectedSessionId!;
    await demoAdapter.startRecording(sessionId, 'microphone');
    const tick = await demoAdapter.dispatch({ type: 'demoTick', sessionId });
    const segment = tick.sessions.find((s) => s.id === sessionId)!.segments.at(-1)!;
    const edit = await demoAdapter.dispatch({ type: 'beginEdit', sessionId, segmentId: segment.id });
    const draft = edit.sessions.find((s) => s.id === sessionId)!.drafts.at(-1)!;
    await demoAdapter.dispatch({ type: 'commitEdit', sessionId, draftId: draft.id, expectedUserSeq: draft.expectedUserSeq, text: 'A carefully corrected statement.' });
    const update = await demoAdapter.dispatch({ type: 'demoTick', sessionId });
    const updated = update.sessions.find((s) => s.id === sessionId)!.segments.find((s) => s.id === segment.id)!;
    expect(updated.displayText).toBe('A carefully corrected statement.');
    expect(updated.pendingMachine).toBeTruthy();
    expect(updated.machineRevision).toBeGreaterThan(segment.machineRevision);
    expect(updated.startSample).toBe(segment.startSample);
  });
  it('does not accept cloud credentials in a browser preview', async () => {
    const { demoAdapter } = await import('./demo');
    await expect(demoAdapter.dispatch({ type: 'configureSoniox', apiKey: 'test-secret' })).rejects.toThrow();
    expect(JSON.stringify(await demoAdapter.dispatch({ type: 'snapshot' }))).not.toContain('test-secret');
  });
  it('moves committed sentence corrections through undo and redo', async () => {
    const { demoAdapter } = await import('./demo');
    const snapshot = await demoAdapter.dispatch({ type: 'snapshot' });
    const sessionId = snapshot.selectedSessionId!;
    const original = snapshot.sessions.find((session) => session.id === sessionId)!.segments[0];
    const edit = await demoAdapter.dispatch({ type: 'beginEdit', sessionId, segmentId: original.id });
    const draft = edit.sessions.find((session) => session.id === sessionId)!.drafts.at(-1)!;
    const committed = await demoAdapter.dispatch({ type: 'commitEdit', sessionId, draftId: draft.id, expectedUserSeq: draft.expectedUserSeq, text: 'A committed correction.' });
    const corrected = committed.sessions.find((session) => session.id === sessionId)!.segments.find((segment) => segment.id === original.id)!;
    const undone = await demoAdapter.dispatch({ type: 'undo', sessionId, segmentId: original.id, expectedUserSeq: corrected.userSeq });
    const afterUndo = undone.sessions.find((session) => session.id === sessionId)!.segments.find((segment) => segment.id === original.id)!;
    expect(afterUndo.displayText).toBe(original.machineText);
    const redone = await demoAdapter.dispatch({ type: 'redo', sessionId, segmentId: original.id, expectedUserSeq: afterUndo.userSeq });
    expect(redone.sessions.find((session) => session.id === sessionId)!.segments.find((segment) => segment.id === original.id)!.displayText).toBe('A committed correction.');
  });
});
