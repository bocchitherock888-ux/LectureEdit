/** UI labels only. Native feature availability comes from runtime_info. */
export function normaliseDesktopPlatform(value: string): 'macos' | 'windows' | 'other' {
  if (/^(mac|darwin)/i.test(value)) return 'macos';
  if (/^win/i.test(value)) return 'windows';
  return 'other';
}

function browserPlatform(): string {
  if (typeof navigator === 'undefined') return '';
  const extended = navigator as Navigator & { userAgentData?: { platform?: string } };
  return extended.userAgentData?.platform || navigator.platform || '';
}

export function shortcutLabel(
  key: string,
  platform: string = browserPlatform(),
  modifier: 'primary' | 'alt' = 'primary',
): string {
  if (normaliseDesktopPlatform(platform) === 'macos') {
    return `${modifier === 'alt' ? '⌥' : '⌘'}${key === 'Enter' ? '↵' : key.toUpperCase()}`;
  }
  return `${modifier === 'alt' ? 'Alt' : 'Ctrl'}+${key === 'Enter' ? 'Enter' : key.toUpperCase()}`;
}

export function canExportNativePdf(platform: string | undefined): boolean {
  return platform === 'macos';
}

export const PDF_PLATFORM_NOTICE = 'Windows 版请先导出课程阅读 HTML，再用 Edge 打开并打印为 PDF。精排 PDF 当前在 macOS 提供。';
