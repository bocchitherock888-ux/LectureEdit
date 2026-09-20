import { describe, expect, it } from 'vitest';
import { normaliseDesktopPlatform, shortcutLabel, canExportNativePdf } from './platform';

describe('Windows portability UI', () => {
  it('recognises Windows and macOS platform names', () => {
    expect(normaliseDesktopPlatform('Win32')).toBe('windows');
    expect(normaliseDesktopPlatform('Windows')).toBe('windows');
    expect(normaliseDesktopPlatform('MacIntel')).toBe('macos');
    expect(normaliseDesktopPlatform('Linux')).toBe('other');
  });
  it('uses Ctrl and Alt on Windows', () => {
    expect(shortcutLabel('e', 'Win32')).toBe('Ctrl+E');
    expect(shortcutLabel('Enter', 'Windows')).toBe('Ctrl+Enter');
    expect(shortcutLabel('Enter', 'Win32', 'alt')).toBe('Alt+Enter');
  });
  it('preserves macOS shortcut glyphs', () => {
    expect(shortcutLabel('k', 'MacIntel')).toBe('⌘K');
    expect(shortcutLabel('Enter', 'macos')).toBe('⌘↵');
    expect(shortcutLabel('Enter', 'MacIntel', 'alt')).toBe('⌥↵');
  });
  it('advertises only the implemented native PDF platform', () => {
    expect(canExportNativePdf('macos')).toBe(true);
    for (const platform of ['windows', 'linux', 'demo', undefined]) {
      expect(canExportNativePdf(platform)).toBe(false);
    }
  });
});
