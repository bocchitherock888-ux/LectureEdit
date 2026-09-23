export const LIVE_TRANSCRIPT_ANCHOR_RATIO = 0.64;

export function transcriptFollowTarget(anchorOffset: number, scrollHeight: number, viewportHeight: number) {
  const maximumScroll = Math.max(0, scrollHeight - viewportHeight);
  const preferredScroll = anchorOffset - viewportHeight * LIVE_TRANSCRIPT_ANCHOR_RATIO;
  return Math.max(0, Math.min(maximumScroll, preferredScroll));
}

export function shouldPauseTranscriptFollow({
  currentScrollTop,
  previousScrollTop,
  currentScrollHeight,
  previousScrollHeight,
  currentViewportHeight,
  previousViewportHeight,
  programmatic,
  pointerDown,
}: {
  currentScrollTop: number;
  previousScrollTop: number;
  currentScrollHeight: number;
  previousScrollHeight: number;
  currentViewportHeight: number;
  previousViewportHeight: number;
  programmatic: boolean;
  pointerDown: boolean;
}) {
  const scrollingUp = currentScrollTop < previousScrollTop - 8;
  const viewportChanged = currentViewportHeight !== previousViewportHeight;
  const contentDidNotShrink = currentScrollHeight >= previousScrollHeight;
  return !programmatic && !viewportChanged && scrollingUp && (contentDidNotShrink || pointerDown);
}

/** Content shrinking by more than this share of the viewport (a deleted segment, a new session) lets the view move back down. */
export const FOLLOW_RESET_RATIO = 0.4;

/**
 * Where live following should settle. Recognition rewrites its latest text, so the transcript's end
 * wobbles by a line or two; following only ever advances, so the page keeps rolling upward
 * instead of bouncing, and only a large shrink moves the view back.
 */
export function nextFollowGoal(previousGoal: number | null, target: number, viewportHeight: number) {
  if (previousGoal === null || target >= previousGoal) return target;
  return previousGoal - target > viewportHeight * FOLLOW_RESET_RATIO ? target : previousGoal;
}

/** One frame of an exponential glide toward `goal`; about 95% of the way after 3 × `tau` ms. */
export function followStep(current: number, goal: number, elapsedMs: number, tau = 120) {
  const next = current + (goal - current) * (1 - Math.exp(-Math.max(0, elapsedMs) / tau));
  return Math.abs(goal - next) < 0.5 ? goal : next;
}
