import { describe, expect, it } from 'vitest';
import { groupTranscriptSegments } from './App';
import type { Segment } from './types';

function segment(id: string, text: string, runId = 'run-1'): Segment {
  return {
    id,
    runId,
    startSample: 0,
    endSample: 16_000,
    machineText: text,
    displayText: text,
    machineRevision: 1,
    workerEpoch: 1,
    userSeq: 0,
    final: true,
    pendingMachine: null,
    corrected: false,
    history: [],
    historyIndex: 0,
  } as Segment;
}

describe('stable transcript paragraph grouping', () => {
  it('removes a stale paragraph boundary immediately after two source segments merge', () => {
    const state = { known: new Set<string>(), paragraphStarts: new Set<string>() };
    const first = segment('first', 'A complete first paragraph that remains in place.');
    const removedBoundary = segment('second', 'A new paragraph starts here.', 'run-2');

    expect(groupTranscriptSegments([first, removedBoundary], state)).toHaveLength(2);
    expect(state.paragraphStarts.has('second')).toBe(true);

    const merged = segment('first', 'A complete first paragraph that remains in place and now includes the continuation.');
    const groups = groupTranscriptSegments([merged], state);

    expect(groups).toHaveLength(1);
    expect(groups[0].map((item) => item.id)).toEqual(['first']);
    expect(state.paragraphStarts.has('second')).toBe(false);
  });
});
