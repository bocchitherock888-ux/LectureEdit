import type { AppState, Session } from './types';

/** What the backend sends: lectures changed since the caller's revision. */
export interface StateDelta {
  epoch: string;
  revision: number;
  full: boolean;
  meta?: Omit<AppState, 'sessions'>;
  order: string[];
  sessions: Session[];
}

export interface SyncedState { state: AppState; epoch: string; revision: number }

/**
 * Folds a delta into the state held by the UI. Unchanged lectures keep their
 * object identity, so React skips them. Returns null when the delta cannot be
 * applied and a full resync is needed.
 */
export function applyDelta(held: SyncedState | null, delta: StateDelta): SyncedState | null {
  if (delta.full || !held || held.epoch !== delta.epoch) {
    if (!delta.full || !delta.meta) return null;
    const byId = new Map(delta.sessions.map((session) => [session.id, session]));
    const sessions = delta.order.map((id) => byId.get(id));
    if (sessions.some((session) => !session)) return null;
    return { state: { ...delta.meta, sessions: sessions as Session[] }, epoch: delta.epoch, revision: delta.revision };
  }
  // A reply that raced a newer one carries nothing the held state lacks.
  if (delta.revision <= held.revision) return held;
  const byId = new Map(held.state.sessions.map((session) => [session.id, session]));
  for (const session of delta.sessions) byId.set(session.id, shareSession(byId.get(session.id), session));
  const sessions = delta.order.map((id) => byId.get(id));
  if (sessions.some((session) => !session)) return null;
  const { sessions: _previous, ...meta } = held.state;
  return { state: { ...(delta.meta ?? meta), sessions: sessions as Session[] }, epoch: delta.epoch, revision: delta.revision };
}

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    const other = b as unknown[];
    return a.length === other.length && a.every((item, index) => same(item, other[index]));
  }
  const left = a as Record<string, unknown>; const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  return keys.length === Object.keys(right).length && keys.every((key) => same(left[key], right[key]));
}

function shareList<T extends { id: string }>(previous: T[], next: T[]): T[] {
  const byId = new Map(previous.map((item) => [item.id, item]));
  let reused = next.length === previous.length;
  const shared = next.map((item, index) => {
    const old = byId.get(item.id);
    if (old && same(old, item)) { if (previous[index] !== old) reused = false; return old; }
    reused = false;
    return item;
  });
  return reused ? previous : shared;
}

/**
 * A changed lecture arrives as fresh JSON. Reuse every sentence and note that
 * did not change so memoised paragraphs skip rendering; only the live tail and
 * edited sentences get new objects.
 */
export function shareSession(previous: Session | undefined, next: Session): Session {
  if (!previous) return next;
  const segments = shareList(previous.segments, next.segments);
  const notes = shareList(previous.notes, next.notes);
  const runs = same(previous.runs, next.runs) ? previous.runs : next.runs;
  const merged = { ...next, segments, notes, runs };
  return same(previous, merged) ? previous : merged;
}
