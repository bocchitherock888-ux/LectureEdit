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
