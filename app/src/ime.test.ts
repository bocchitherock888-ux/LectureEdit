import { describe, expect, it } from 'vitest';
import { imeActive } from './ime';

describe('imeActive', () => {
  it('treats composing keystrokes and the WebKit commit key as IME input', () => {
    expect(imeActive({ isComposing: true, keyCode: 13 })).toBe(true);
    expect(imeActive({ isComposing: false, keyCode: 229 })).toBe(true);
    expect(imeActive({ isComposing: false, keyCode: 13 })).toBe(false);
    expect(imeActive({})).toBe(false);
  });
});
