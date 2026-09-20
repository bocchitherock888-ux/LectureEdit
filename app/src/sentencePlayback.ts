export type SentencePlaybackSlice = {
  text: string;
  startOffset: number;
  endOffset: number;
  startSample: number;
  endSample: number;
};

type TextRange = Pick<SentencePlaybackSlice, 'text' | 'startOffset' | 'endOffset'>;

const MAX_SLICE_LENGTH = 240;
const TARGET_SLICE_LENGTH = 170;
const MIN_TAIL_LENGTH = 64;

function segmentRanges(text: string): TextRange[] {
  if (!text) return [];
  const ranges: TextRange[] = [];
  if (typeof Intl.Segmenter === 'function') {
    const segmenter = new Intl.Segmenter(undefined, { granularity: 'sentence' });
    for (const part of segmenter.segment(text)) {
      const startOffset = part.index;
      const endOffset = startOffset + part.segment.length;
      ranges.push({ text: part.segment, startOffset, endOffset });
    }
  } else {
    const expression = /.+?(?:[.!?。！？]+[\u201d\u2019\u300d\u300f)\]]*\s+|$)/gs;
    for (const match of text.matchAll(expression)) {
      const startOffset = match.index ?? 0;
      ranges.push({ text: match[0], startOffset, endOffset: startOffset + match[0].length });
    }
  }
  return ranges.length ? ranges : [{ text, startOffset: 0, endOffset: text.length }];
}

function splitLongRange(range: TextRange): TextRange[] {
  if (range.text.trim().length <= MAX_SLICE_LENGTH) return [range];
  const output: TextRange[] = [];
  let cursor = 0;
  while (range.text.length - cursor > MAX_SLICE_LENGTH) {
    const maximum = Math.min(cursor + MAX_SLICE_LENGTH, range.text.length - MIN_TAIL_LENGTH);
    const ideal = Math.min(maximum, cursor + TARGET_SLICE_LENGTH);
    const minimum = cursor + Math.floor(TARGET_SLICE_LENGTH * .55);
    let split = -1;
    for (let distance = 0; distance <= maximum - minimum; distance += 1) {
      for (const index of [ideal + distance, ideal - distance]) {
        if (index < minimum || index > maximum) continue;
        if (/[\s,;:\u2014\u2013\uff0c\uff1b\uff1a]/u.test(range.text[index] ?? '')) { split = index + 1; break; }
      }
      if (split >= minimum) break;
    }
    if (split < minimum) split = maximum;
    const startOffset = range.startOffset + cursor;
    const endOffset = range.startOffset + split;
    output.push({ text: range.text.slice(cursor, split), startOffset, endOffset });
    cursor = split;
  }
  if (cursor < range.text.length) output.push({ text: range.text.slice(cursor), startOffset: range.startOffset + cursor, endOffset: range.endOffset });
  return output;
}

function spokenWeight(text: string) {
  const useful = Array.from(text).filter((character) => /[\p{L}\p{N}\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]/u.test(character)).length;
  return Math.max(1, useful);
}

export function sentencePlaybackSlices(text: string, startSample: number, endSample: number): SentencePlaybackSlice[] {
  const ranges = segmentRanges(text).flatMap(splitLongRange).filter((range) => range.text.trim().length > 0);
  if (!ranges.length) return [];
  const safeStart = Math.max(0, Math.floor(startSample));
  const safeEnd = Math.max(safeStart, Math.floor(endSample));
  const duration = safeEnd - safeStart;
  const weights = ranges.map((range) => spokenWeight(range.text));
  const totalWeight = weights.reduce((sum, weight) => sum + weight, 0);
  let consumedWeight = 0;
  let cursor = safeStart;
  return ranges.map((range, index) => {
    consumedWeight += weights[index];
    const last = index === ranges.length - 1;
    const proportional = safeStart + Math.round(duration * consumedWeight / totalWeight);
    const remaining = ranges.length - index - 1;
    const boundary = last ? safeEnd : Math.min(safeEnd - remaining, Math.max(cursor + 1, proportional));
    const slice = { ...range, startSample: cursor, endSample: Math.max(cursor, boundary) };
    cursor = slice.endSample;
    return slice;
  });
}
