import { describe, expect, it } from 'vitest';
import { followStep, LIVE_TRANSCRIPT_ANCHOR_RATIO, nextFollowGoal, shouldPauseTranscriptFollow, transcriptFollowTarget } from './transcriptFollow';

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

describe('live transcript follow motion', () => {
  it('only moves forward while the latest text is rewritten', () => {
    let goal = nextFollowGoal(null, 1000, 800);
    goal = nextFollowGoal(goal, 1030, 800);
    expect(goal).toBe(1030);
    // A partial result is replaced by a shorter final one: stay put instead of jumping back.
    goal = nextFollowGoal(goal, 1000, 800);
    expect(goal).toBe(1030);
    goal = nextFollowGoal(goal, 1060, 800);
    expect(goal).toBe(1060);
  });

  it('moves back when the transcript shrinks a lot', () => {
    expect(nextFollowGoal(1060, 500, 800)).toBe(500);
  });

  it('glides toward the goal without overshooting', () => {
    let position = 0;
    for (let frame = 0; frame < 6; frame += 1) {
      const next = followStep(position, 30, 16);
      expect(next).toBeGreaterThan(position);
      expect(next).toBeLessThanOrEqual(30);
      position = next;
    }
    for (let frame = 0; frame < 60; frame += 1) position = followStep(position, 30, 16);
    expect(position).toBe(30);
  });
});
