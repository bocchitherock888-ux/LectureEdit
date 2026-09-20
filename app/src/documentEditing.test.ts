import { beforeEach, describe, expect, it, vi } from 'vitest';
import { changedTranscriptSegments, persistTranscriptEdits, transcriptTextMap } from './documentEditing';

describe('full transcript editing', () => {
  beforeEach(() => vi.resetModules());

  it('selects only segments whose text changed', async () => {
    const { demoAdapter } = await import('./demo');
    const snapshot = await demoAdapter.dispatch({ type: 'snapshot' });
    const session = snapshot.sessions.find((item) => item.id === snapshot.selectedSessionId)!;
    const original = transcriptTextMap(session.segments);
    const current = { ...original, [session.segments[1].id]: 'A revised explanation of opportunity cost.' };
    expect(changedTranscriptSegments(session.segments, original, current).map((segment) => segment.id)).toEqual([session.segments[1].id]);
  });

  it('persists changed text while preserving segment and audio anchors', async () => {
    const { demoAdapter } = await import('./demo');
    const snapshot = await demoAdapter.dispatch({ type: 'snapshot' });
    const session = snapshot.sessions.find((item) => item.id === snapshot.selectedSessionId)!;
    const target = session.segments[1];
    const original = transcriptTextMap(session.segments);
    const current = { ...original, [target.id]: 'Opportunity cost is the value of the best available alternative.' };
    const result = await persistTranscriptEdits(demoAdapter, session.id, session.segments, original, current);
    const updated = result.state!.sessions.find((item) => item.id === session.id)!.segments.find((segment) => segment.id === target.id)!;
    expect(result.savedSegmentIds).toEqual([target.id]);
    expect(updated.displayText).toBe(current[target.id]);
    expect(updated.id).toBe(target.id);
    expect(updated.runId).toBe(target.runId);
    expect(updated.startSample).toBe(target.startSample);
    expect(updated.endSample).toBe(target.endSample);
  });
});
