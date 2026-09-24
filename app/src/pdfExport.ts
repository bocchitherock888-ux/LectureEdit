import katex from 'katex';
import type { Note, Segment, Session } from './types';

const EXPORT_ROOT_ID = 'lectureedit-pdf-export';
const PRINTING_CLASS = 'lectureedit-pdf-printing';
const CAPTURE_CLASS = 'lectureedit-pdf-capture';

export type PdfPageRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export const PDF_STYLES = `
@page {
  size: A4 portrait;
  margin: 21mm 18mm 18mm;
  @top-left {
    content: string(course-title);
    color: #66706f;
    font: 500 8.5pt -apple-system, BlinkMacSystemFont, "PingFang SC", "Hiragino Sans GB", sans-serif;
  }
  @top-right {
    content: string(course-date);
    color: #7a8281;
    font: 400 8.5pt -apple-system, BlinkMacSystemFont, "PingFang SC", "Hiragino Sans GB", sans-serif;
  }
  @bottom-center {
    content: "0 / 0";
    color: #7a8281;
    font: 500 8.5pt -apple-system, BlinkMacSystemFont, "PingFang SC", "Hiragino Sans GB", sans-serif;
  }
}
@page:first {
  @top-left { content: none; }
  @top-right { content: none; }
}
.pdf-document {
  color: #1e2625;
  font: 11pt/1.72 -apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif;
  letter-spacing: .005em;
}
.pdf-title-page {
  min-height: 72mm;
  padding: 12mm 0 10mm;
  border-bottom: .35mm solid #163d38;
  margin-bottom: 11mm;
}
.pdf-kicker {
  margin: 0 0 7mm;
  color: #3f7068;
  font-size: 8.5pt;
  font-weight: 700;
  letter-spacing: .14em;
  text-transform: uppercase;
}
.pdf-course-title {
  string-set: course-title content(text);
  max-width: 155mm;
  margin: 0;
  color: #102d29;
  font: 650 29pt/1.16 -apple-system, BlinkMacSystemFont, "SF Pro Display", "PingFang SC", "Hiragino Sans GB", sans-serif;
  letter-spacing: -.025em;
}
.pdf-course-date {
  string-set: course-date content(text);
  margin: 8mm 0 0;
  color: #687371;
  font-size: 10pt;
}
.pdf-transcript-group { margin: 0 0 5.2mm; }
.pdf-transcript {
  margin: 0;
  white-space: pre-wrap;
  orphans: 3;
  widows: 3;
}
.pdf-note {
  break-inside: avoid;
  margin: 4mm 0 5mm 5mm;
  padding: 3mm 0 2.5mm;
  border-top: .25mm solid #cfd8d5;
  background: #fff;
}
.pdf-note-label {
  margin: 0 0 1.5mm;
  color: #37665f;
  font-size: 8.3pt;
  font-weight: 700;
  letter-spacing: .04em;
  text-transform: uppercase;
}
.pdf-note p { margin: 0; white-space: pre-wrap; }
.pdf-formula { overflow-wrap: anywhere; text-align: center; font-size: 12pt; }
.pdf-formula .katex-display { max-width: 100%; overflow: visible; margin: 1mm 0; }
.pdf-formula .katex { display: inline-block; white-space: nowrap; transform-origin: center top; }
.pdf-formula-source { margin-top: 2mm !important; overflow-wrap: anywhere; word-break: break-word; white-space: pre-wrap !important; color: #65616e; font: 8.5pt/1.45 ui-monospace, "SFMono-Regular", Menlo, monospace; }
.pdf-note img {
  display: block;
  max-width: 100%;
  max-height: 125mm;
  margin: 1mm auto 2mm;
  object-fit: contain;
}
.pdf-empty { color: #7b8382; font-style: italic; }
`;

export const PDF_CONTROL_STYLES = `
#${EXPORT_ROOT_ID} {
  position: fixed;
  left: -300vw;
  top: 0;
  width: 210mm;
  opacity: 0;
  pointer-events: none;
  z-index: -2147483648;
}
#${EXPORT_ROOT_ID} .pagedjs_margin-bottom-center .pagedjs_margin-content::after {
  content: attr(data-pdf-page-label) !important;
}
#${EXPORT_ROOT_ID} > .pdf-export-staging {
  width: 174mm;
  font-size: 12pt;
}
#${EXPORT_ROOT_ID} > .pdf-export-staging .pdf-formula { font-size: 12pt; text-align: center; }
#${EXPORT_ROOT_ID} > .pdf-export-staging img { display: block; max-width: 100%; max-height: 125mm; object-fit: contain; }
html.${CAPTURE_CLASS}, html.${CAPTURE_CLASS} body {
  width: 210mm !important;
  height: auto !important;
  min-width: 0 !important;
  min-height: 0 !important;
  margin: 0 !important;
  padding: 0 !important;
  overflow: visible !important;
  background: #fff !important;
}
html.${CAPTURE_CLASS} body > * {
  display: none !important;
}
html.${CAPTURE_CLASS} body > #${EXPORT_ROOT_ID} {
  display: block !important;
  position: static !important;
  width: 210mm !important;
  height: auto !important;
  min-height: 0 !important;
  max-height: none !important;
  opacity: 1 !important;
  overflow: visible !important;
  pointer-events: none !important;
  z-index: auto !important;
}
html.${CAPTURE_CLASS} #${EXPORT_ROOT_ID} .pagedjs_pages {
  display: block !important;
  width: 210mm !important;
  height: auto !important;
  min-height: 0 !important;
  max-height: none !important;
  margin: 0 !important;
  padding: 0 !important;
  overflow: visible !important;
  transform: none !important;
}
html.${CAPTURE_CLASS} #${EXPORT_ROOT_ID} .pagedjs_page {
  width: 210mm !important;
  height: 297mm !important;
  min-height: 297mm !important;
  max-height: 297mm !important;
  margin: 0 !important;
  padding: 0 !important;
  border: 0 !important;
  box-shadow: none !important;
  overflow: hidden !important;
}
html.${CAPTURE_CLASS} #${EXPORT_ROOT_ID} .pagedjs_sheet,
html.${CAPTURE_CLASS} #${EXPORT_ROOT_ID} .pagedjs_pagebox {
  width: 210mm !important;
  height: 297mm !important;
  min-height: 297mm !important;
  max-height: 297mm !important;
}
@media print {
  html.${PRINTING_CLASS}, html.${PRINTING_CLASS} body {
    width: 210mm !important;
    height: auto !important;
    min-width: 0 !important;
    max-width: none !important;
    min-height: 0 !important;
    max-height: none !important;
    margin: 0 !important;
    padding: 0 !important;
    overflow: visible !important;
    background: #fff !important;
  }
  body > * { display: none !important; }
  body > #${EXPORT_ROOT_ID} {
    display: block !important;
    position: static !important;
    width: 210mm !important;
    height: auto !important;
    min-height: 0 !important;
    max-height: none !important;
    opacity: 1 !important;
    overflow: visible !important;
    pointer-events: auto !important;
  }
  #${EXPORT_ROOT_ID} .pagedjs_pages {
    display: block !important;
    width: 210mm !important;
    height: auto !important;
    min-height: 0 !important;
    max-height: none !important;
    overflow: visible !important;
    transform: none !important;
  }
  #${EXPORT_ROOT_ID} .pagedjs_page {
    width: 210mm !important;
    height: 297mm !important;
    min-height: 297mm !important;
    max-height: 297mm !important;
    margin: 0 !important;
    padding: 0 !important;
    border: 0 !important;
    box-shadow: none !important;
    overflow: hidden !important;
    page-break-inside: avoid !important;
    break-inside: avoid !important;
    page-break-after: always !important;
    break-after: page !important;
  }
  #${EXPORT_ROOT_ID} .pagedjs_sheet,
  #${EXPORT_ROOT_ID} .pagedjs_pagebox {
    width: 210mm !important;
    height: 297mm !important;
    min-height: 297mm !important;
    max-height: 297mm !important;
  }
  #${EXPORT_ROOT_ID} .pagedjs_page:last-child {
    page-break-after: auto !important;
    break-after: auto !important;
  }
}
`;

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[character]!));
}

function safeImage(value: string | null) {
  return value && /^data:image\/(?:png|jpeg|webp);base64,[a-z0-9+/=\s]+$/i.test(value) ? value : null;
}

function groupSegments(segments: Segment[]) {
  const groups: Segment[][] = [];
  for (const segment of segments) {
    const current = groups.at(-1);
    const length = current?.reduce((sum, item) => sum + item.displayText.length, 0) ?? 0;
    const previous = current?.at(-1);
    const naturalBreak = Boolean(previous && /[.!?。！？][”’」』]?\s*$/.test(previous.displayText));
    if (!current || previous?.runId !== segment.runId || length >= 520 || (length >= 300 && naturalBreak)) groups.push([segment]);
    else current.push(segment);
  }
  return groups;
}

function noteLabel(note: Note) {
  const fallback = note.kind === 'formula' ? 'Formula' : note.kind === 'example' ? 'Example' : note.kind === 'image' ? 'Image' : 'Note';
  return note.sourceLabel?.trim() || fallback;
}

function renderFormula(latex: string) {
  try {
    return katex.renderToString(latex, { displayMode: true, output: 'html', throwOnError: true, trust: false, maxExpand: 1000 });
  } catch {
    return '';
  }
}

function renderNote(note: Note) {
  const label = `<div class="pdf-note-label">${escapeHtml(noteLabel(note))}</div>`;
  if (note.kind === 'formula') {
    const formula = renderFormula(note.text);
    if (formula) return `<aside class="pdf-note pdf-note-formula">${label}<div class="pdf-formula">${formula}</div></aside>`;
    return `<aside class="pdf-note pdf-note-formula pdf-note-formula-fallback">${label}<p class="pdf-formula-source">${escapeHtml(note.text)}</p></aside>`;
  }
  if (note.kind === 'image') {
    const image = safeImage(note.imageData);
    const media = image ? `<img src="${escapeHtml(image)}" alt="${escapeHtml(note.text || 'Course image')}">` : '';
    const caption = note.text ? `<p>${escapeHtml(note.text)}</p>` : '';
    return `<aside class="pdf-note pdf-note-image">${label}${media}${caption}</aside>`;
  }
  return `<aside class="pdf-note pdf-note-${note.kind}">${label}<p>${escapeHtml(note.text)}</p></aside>`;
}

export function buildPdfContent(session: Session, locale = 'en') {
  const date = new Date(session.createdAt);
  const dateText = Number.isNaN(date.getTime())
    ? ''
    : new Intl.DateTimeFormat(locale, { year: 'numeric', month: 'long', day: 'numeric' }).format(date);
  const groups = groupSegments(session.segments);
  const transcript = groups.map((group) => {
    const ids = new Set(group.map((segment) => segment.id));
    const text = group
      .map((segment) => `<span class="pdf-transcript-segment" data-segment-id="${escapeHtml(segment.id)}">${escapeHtml(segment.displayText)}</span>`)
      .join(' ');
    const notes = session.notes.filter((note) => ids.has(note.segmentId)).map(renderNote).join('');
    return `<section class="pdf-transcript-group"><p class="pdf-transcript">${text}</p>${notes}</section>`;
  }).join('');
  const body = transcript || '<p class="pdf-empty">No transcript content.</p>';
  return `<article class="pdf-document"><header class="pdf-title-page"><p class="pdf-kicker">随堂 · Course transcript</p><h1 class="pdf-course-title">${escapeHtml(session.title)}</h1><p class="pdf-course-date"><time datetime="${Number.isNaN(date.getTime()) ? '' : date.toISOString()}">${escapeHtml(dateText)}</time></p></header><main>${body}</main></article>`;
}

async function settleImages(root: HTMLElement) {
  await Promise.all(Array.from(root.querySelectorAll('img')).map(async (image) => {
    if (image.complete) return;
    try { await image.decode(); } catch { /* The image remains represented by its caption. */ }
  }));
}

function fitFormulas(root: HTMLElement) {
  const minimumSize = 7.5;
  for (const container of root.querySelectorAll<HTMLElement>('.pdf-formula')) {
    const formula = container.querySelector<HTMLElement>('.katex');
    if (!formula || container.clientWidth <= 0) continue;
    formula.style.transform = '';
    container.style.fontSize = '12pt';
    let size = 12;
    while (formula.scrollWidth > container.clientWidth && size > minimumSize) {
      size = Math.max(minimumSize, size - 0.5);
      container.style.fontSize = `${size}pt`;
    }
    if (formula.scrollWidth > container.clientWidth) {
      const scale = container.clientWidth / formula.scrollWidth;
      formula.style.transform = `scale(${scale})`;
    }
  }
}

function numberPages(root: HTMLElement) {
  const renderedPages = Array.from(root.querySelectorAll<HTMLElement>('.pagedjs_page'));
  for (const [index, page] of renderedPages.entries()) {
    page.querySelector<HTMLElement>('.pagedjs_margin-bottom-center .pagedjs_margin-content')
      ?.setAttribute('data-pdf-page-label', `${index + 1} / ${renderedPages.length}`);
  }
}

function capturePageRects(root: HTMLElement): PdfPageRect[] {
  return Array.from(root.querySelectorAll<HTMLElement>('.pagedjs_page')).map((page) => {
    const bounds = page.getBoundingClientRect();
    return {
      x: bounds.left + window.scrollX,
      y: bounds.top + window.scrollY,
      width: bounds.width,
      height: bounds.height,
    };
  });
}

export async function preparePdfExport(session: Session) {
  if (document.getElementById(EXPORT_ROOT_ID)) throw new Error('另一份 PDF 正在生成，请稍候。');
  const root = document.createElement('div');
  root.id = EXPORT_ROOT_ID;
  root.setAttribute('aria-hidden', 'true');
  const pages = document.createElement('div');
  root.appendChild(pages);
  const staging = document.createElement('div');
  staging.className = 'pdf-export-staging';
  staging.innerHTML = buildPdfContent(session, navigator.language || 'en');
  root.appendChild(staging);
  const control = document.createElement('style');
  control.dataset.lectureeditPdf = 'true';
  control.textContent = PDF_CONTROL_STYLES;
  document.head.appendChild(control);
  document.body.appendChild(root);
  const scrollX = window.scrollX;
  const scrollY = window.scrollY;

  let previewer: import('pagedjs').Previewer | null = null;
  let cleaned = false;
  const cleanup = () => {
    if (cleaned) return;
    cleaned = true;
    try { previewer?.chunker.destroy(); } catch { /* Root removal below still discards rendered pages. */ }
    try { previewer?.polisher.destroy(); } catch { /* The control style is removed below. */ }
    root.remove();
    control.remove();
    document.documentElement.classList.remove(PRINTING_CLASS);
    document.documentElement.classList.remove(CAPTURE_CLASS);
    window.scrollTo(scrollX, scrollY);
  };

  try {
    const { Previewer } = await import('pagedjs');
    previewer = new Previewer();
    if ('fonts' in document) await document.fonts.ready;
    await settleImages(staging);
    fitFormulas(staging);
    const content = staging.innerHTML;
    staging.remove();
    const flow = await previewer.preview(
      content,
      [{ [`${window.location.href}#lectureedit-pdf`]: PDF_STYLES }],
      pages,
    );
    if ('fonts' in document) await document.fonts.ready;
    await settleImages(root);
    fitFormulas(root);
    numberPages(root);
    document.documentElement.classList.add(CAPTURE_CLASS);
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    if (!flow.total || !root.querySelector('.pagedjs_page')) throw new Error('课程内容无法分页。');
    const pageRects = capturePageRects(root);
    if (pageRects.length !== flow.total) throw new Error('课程分页结果不完整。');
    return { pageCount: flow.total, pageRects, cleanup };
  } catch (error) {
    cleanup();
    throw error;
  }
}
