import { describe, expect, it } from 'vitest';
import { buildPdfContent, PDF_CONTROL_STYLES, PDF_STYLES } from './pdfExport';
import type { Session } from './types';

const fixture = (): Session => ({
  id: 'course-1', projectId: null, title: 'Economics <101> & 经济学', createdAt: Date.UTC(2026, 8, 19), mode: 'live',
  recordingState: 'stopped', inferenceState: 'ready', drafts: [], runs: [], gaps: [], operationSeq: 1, error: null,
  segments: [
    { id: 's1', runId: 'r1', startSample: 0, endSample: 10, machineText: 'Supply', machineRevision: 1, final: true, workerEpoch: 1, userSeq: 0, displayText: 'Supply < demand.', pendingMachine: null, corrected: false, history: [], historyIndex: 0 },
    { id: 's2', runId: 'r1', startSample: 10, endSample: 20, machineText: '价格上升。', machineRevision: 1, final: true, workerEpoch: 1, userSeq: 0, displayText: '价格上升。', pendingMachine: null, corrected: false, history: [], historyIndex: 0 },
  ],
  notes: [
    { id: 'n1', segmentId: 's1', runId: 'r1', sample: 2, kind: 'note', text: '<script>alert(1)</script>', sourceLabel: 'Lecturer & slides', imageData: null },
    { id: 'n2', segmentId: 's2', runId: 'r1', sample: 12, kind: 'formula', text: 'E = mc^2', sourceLabel: null, imageData: null },
    { id: 'n3', segmentId: 's2', runId: 'r1', sample: 13, kind: 'image', text: 'Curve "A"', sourceLabel: null, imageData: 'data:image/png;base64,aGVsbG8=' },
  ],
});

describe('PDF course document', () => {
  it('escapes course content while retaining bilingual text and valid images', () => {
    const html = buildPdfContent(fixture(), 'en');
    expect(html).toContain('Economics &lt;101&gt; &amp; 经济学');
    expect(html).toContain('Supply &lt; demand.');
    expect(html).toContain('价格上升。');
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
    expect(html).not.toContain('<script>alert(1)</script>');
    expect(html).toContain('data:image/png;base64,aGVsbG8=');
    expect(html).toContain('Supply &lt; demand.</span> <span class="pdf-transcript-segment"');
  });

  it('renders formulas as KaTeX HTML without a fragmentable MathML source annotation', () => {
    const html = buildPdfContent(fixture(), 'en');
    expect(html).toContain('class="katex"');
    expect(html).toContain('class="katex-html"');
    expect(html).not.toContain('<annotation');
    expect(html).not.toContain('class="pdf-formula-source"');
  });

  it('keeps an escaped, wrapping fallback only when LaTeX is invalid', () => {
    const session = fixture();
    session.notes[1] = { ...session.notes[1], text: '\\notacommand{' };
    const html = buildPdfContent(session, 'en');
    expect(html).toContain('pdf-note-formula-fallback');
    expect(html).toContain('class="pdf-formula-source"');
    expect(html).toContain('\\notacommand{');
  });

  it('defines A4 pagination, running metadata, and page counts', () => {
    expect(PDF_STYLES).toContain('size: A4 portrait');
    expect(PDF_STYLES).toContain('string(course-title)');
    expect(PDF_STYLES).not.toContain('counter(pages)');
    expect(PDF_STYLES).not.toContain('.pdf-formula { overflow: hidden');
    expect(PDF_STYLES).toContain('word-break: break-word');
    expect(PDF_CONTROL_STYLES).toContain('height: auto !important');
    expect(PDF_CONTROL_STYLES).toContain('height: 297mm !important');
    expect(PDF_CONTROL_STYLES).toContain('page-break-after: always !important');
    expect(PDF_CONTROL_STYLES).toContain('lectureedit-pdf-capture');
    expect(PDF_CONTROL_STYLES).toContain('body > #lectureedit-pdf-export');
  });
});
