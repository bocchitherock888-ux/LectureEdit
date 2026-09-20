import { describe, expect, it } from 'vitest';
import { LIVE_TRANSCRIPT_ANCHOR_RATIO, shouldPauseTranscriptFollow, transcriptFollowTarget } from './transcriptFollow';

describe('live transcript follow position', () => {
  it.each([760, 1024, 1440])('keeps the live anchor at 64%% in a %ipx viewport', (viewportHeight) => {
    const anchorOffset = 3200;
    const scrollHeight = anchorOffset + viewportHeight * 0.38;
    const scrollTop = transcriptFollowTarget(anchorOffset, scrollHeight, viewportHeight);

    expect((anchorOffset - scrollTop) / viewportHeight).toBeCloseTo(LIVE_TRANSCRIPT_ANCHOR_RATIO, 5);
  });

  it('keeps short transcripts at the top', () => {
    expect(transcriptFollowTarget(300, 700, 760)).toBe(0);
  });

  it('stays within the available scroll range when bottom space is limited', () => {
    expect(transcriptFollowTarget(1600, 1800, 1000)).toBe(800);
  });

  it('pauses when the user scrolls upward', () => {
    expect(shouldPauseTranscriptFollow({
      currentScrollTop: 900,
      previousScrollTop: 1100,
      currentScrollHeight: 2200,
      previousScrollHeight: 2200,
      currentViewportHeight: 760,
      previousViewportHeight: 760,
      programmatic: false,
      pointerDown: false,
    })).toBe(true);
  });

  it('continues following when a taller viewport clamps the scroll position', () => {
    expect(shouldPauseTranscriptFollow({
      currentScrollTop: 900,
      previousScrollTop: 1100,
      currentScrollHeight: 2300,
      previousScrollHeight: 2200,
      currentViewportHeight: 1024,
      previousViewportHeight: 760,
      programmatic: false,
      pointerDown: false,
    })).toBe(false);
  });
});
