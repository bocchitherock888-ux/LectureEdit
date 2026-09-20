import { describe, expect, it } from 'vitest';
import { sentencePlaybackSlices } from './sentencePlayback';

describe('sentence playback anchors', () => {
  it('splits natural English sentences and covers the original audio range', () => {
    const slices = sentencePlaybackSlices('First idea. A considerably longer second idea follows. Final point!', 1_600, 17_600);
    expect(slices.map((slice) => slice.text.trim())).toEqual(['First idea.', 'A considerably longer second idea follows.', 'Final point!']);
    expect(slices[0].startSample).toBe(1_600);
    expect(slices.at(-1)?.endSample).toBe(17_600);
    expect(slices[1].endSample - slices[1].startSample).toBeGreaterThan(slices[0].endSample - slices[0].startSample);
    expect(slices.every((slice, index) => index === 0 || slice.startSample === slices[index - 1].endSample)).toBe(true);
  });

  it('handles Chinese sentence boundaries', () => {
    const slices = sentencePlaybackSlices('价格上升。需求下降！然后市场达到新的均衡。', 0, 48_000);
    expect(slices.map((slice) => slice.text.trim())).toEqual(['价格上升。', '需求下降！', '然后市场达到新的均衡。']);
  });

  it('breaks an unpunctuated run-on transcript into playable phrases without losing text', () => {
    const text = Array.from({ length: 42 }, (_, index) => `word${index}`).join(' ');
    const slices = sentencePlaybackSlices(text, 0, 160_000);
    expect(slices.length).toBeGreaterThan(1);
    expect(slices.map((slice) => slice.text).join('')).toBe(text);
    expect(slices.at(-1)?.endSample).toBe(160_000);
  });
});
