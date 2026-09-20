import type { AppState, LectureAdapter, Segment } from './types';
import { commandId } from './types';

export type TranscriptTextMap = Record<string, string>;

export function transcriptTextMap(segments: Segment[]): TranscriptTextMap {
  return Object.fromEntries(segments.map((segment) => [segment.id, segment.displayText]));
}

export function changedTranscriptSegments(segments: Segment[], original: TranscriptTextMap, current: TranscriptTextMap) {
  return segments.filter((segment) => current[segment.id] !== undefined && current[segment.id] !== original[segment.id]);
}

export class TranscriptSaveError extends Error {
  constructor(message: string, readonly latestState: AppState | null, readonly savedSegmentIds: string[]) {
    super(message);
    this.name = 'TranscriptSaveError';
  }
}

export async function persistTranscriptEdits(
  lectureAdapter: LectureAdapter,
  sessionId: string,
  segments: Segment[],
  original: TranscriptTextMap,
  current: TranscriptTextMap,
  onSaved?: (segmentId: string, state: AppState) => void,
) {
  const changed = changedTranscriptSegments(segments, original, current);
  let latestState: AppState | null = null;
  const savedSegmentIds: string[] = [];
  try {
    for (const segment of changed) {
      latestState = await lectureAdapter.dispatch({ type: 'beginEdit', commandId: commandId(), sessionId, segmentId: segment.id });
      const session = latestState.sessions.find((item) => item.id === sessionId);
      const candidates = session?.drafts.filter((draft) => draft.segmentId === segment.id && !['committed', 'discarded', 'closed'].includes(draft.state)) ?? [];
      const draft = candidates.at(-1);
      if (!draft) throw new Error('无法为这句话建立本地草稿。');
      latestState = await lectureAdapter.dispatch({ type: 'saveDraft', commandId: commandId(), sessionId, draftId: draft.id, text: current[segment.id], revision: draft.revision + 1 });
      latestState = await lectureAdapter.dispatch({ type: 'commitEdit', commandId: commandId(), sessionId, draftId: draft.id, text: current[segment.id], expectedUserSeq: draft.expectedUserSeq });
      savedSegmentIds.push(segment.id);
      onSaved?.(segment.id, latestState);
    }
    return { state: latestState, savedSegmentIds };
  } catch (cause) {
    throw new TranscriptSaveError(cause instanceof Error ? cause.message : String(cause), latestState, savedSegmentIds);
  }
}
