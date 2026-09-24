import { imeActive } from './ime';
import { createContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { Check, CircleAlert, KeyRound, Languages, LoaderCircle, ScanText, X } from 'lucide-react';
import katex from 'katex';
import { adapter } from './bridge';
import type { Segment } from './types';
import './deepseek.css';

export function DeepSeekConnection({ configured, onConfigured }: { configured: boolean; onConfigured?: (configured: boolean) => void }) {
  const [key, setKey] = useState('');
  const [ready, setReady] = useState(configured);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const pending = useRef(false);
  useEffect(() => setReady(configured), [configured]);
  const save = async (clear = false) => {
    if (pending.current) return;
    if (adapter.mode !== 'native') { setError('请在桌面应用中配置 DeepSeek。'); return; }
    pending.current = true; setBusy(true); setError('');
    try {
      await adapter.dispatch({ type: 'configureDeepSeek', apiKey: clear ? '' : key.trim() });
      setReady(!clear); setKey(''); onConfigured?.(!clear);
    } catch (cause) { setError(String(cause)); }
    finally { pending.current = false; setBusy(false); }
  };
  return <section className="deepseek-connection" aria-label="DeepSeek 连接">
    <div className="ai-connection-heading"><span><KeyRound size={16} /> DeepSeek</span><small>{ready ? '已配置' : 'V4.1 Flash'}</small></div>
    <p>轻度整理已确认的转写，也可翻译选中文字或识别图片公式。</p>
    <div className="ai-key-row"><input aria-label="DeepSeek API Key" type="password" autoComplete="off" spellCheck={false} value={key} disabled={busy || adapter.mode === 'demo'} placeholder={ready ? '填写新 Key 可替换' : '粘贴 DeepSeek API Key'} onChange={(event) => setKey(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); event.stopPropagation(); if (!imeActive(event.nativeEvent) && key.trim()) void save(); } }} /><button type="button" className="secondary-button" disabled={busy || !key.trim() || adapter.mode === 'demo'} onClick={() => void save()}>{busy ? '保存中…' : '保存 Key'}</button></div>
    <p className="ai-disclosure">翻译和公式识别只发送当前选择的内容；启用轻度整理后，已确认的转写段落会在后台发送。费用计入你的账户，Key 安全保存在系统凭据中。</p>
    {ready && <button type="button" className="text-button" disabled={busy} onClick={() => void save(true)}>删除已保存 Key</button>}
    {error && <p className="inline-error" role="alert">{error}</p>}
  </section>;
}

function AssistDialog({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  const panel = useRef<HTMLElement>(null);
  const closeAction = useRef(onClose);
  closeAction.current = onClose;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.focus();
    const keyboard = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !imeActive(event)) { event.stopPropagation(); closeAction.current(); }
      if (event.key !== 'Tab') return;
      const controls = [...(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select, textarea:not(:disabled)') ?? [])].filter((item) => item.getClientRects().length);
      const first = controls[0], last = controls.at(-1);
      if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel.current)) { event.preventDefault(); first?.focus(); }
    };
    document.addEventListener('keydown', keyboard);
    return () => { document.removeEventListener('keydown', keyboard); previous?.focus(); };
  }, []);
  return createPortal(<div className="dialog-layer" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}><section ref={panel} tabIndex={-1} className="dialog ai-dialog" role="dialog" aria-modal="true" aria-label="翻译所选文字">{children}</section></div>, document.body);
}

type SelectionAnchor = { text: string; segmentId: string; x: number; y: number };

function normalizedTranscriptText(value: string) {
  return value.replace(/\s+/g, ' ').trim();
}

function transcriptSelection(root: HTMLElement, range: Range) {
  const sentenceNodes = [...root.querySelectorAll<HTMLElement>('.segment-copy:not(.segment-ghost)')]
    .filter((node) => range.intersectsNode(node));
  const firstSentence = sentenceNodes[0];
  const segmentId = firstSentence?.closest<HTMLElement>('[data-segment]')?.dataset.segment;
  if (!segmentId) return null;
  const text = normalizedTranscriptText(sentenceNodes.map((node) => {
    const nodeRange = document.createRange();
    nodeRange.selectNodeContents(node);
    const piece = range.cloneRange();
    if (piece.compareBoundaryPoints(Range.START_TO_START, nodeRange) < 0) piece.setStart(nodeRange.startContainer, nodeRange.startOffset);
    if (piece.compareBoundaryPoints(Range.END_TO_END, nodeRange) > 0) piece.setEnd(nodeRange.endContainer, nodeRange.endOffset);
    return piece.cloneContents().textContent ?? '';
  }).join(' '));
  return text ? { text, segmentId } : null;
}

function selectionSignature(selection: Selection | null) {
  if (!selection || selection.isCollapsed || !selection.rangeCount) return '';
  const range = selection.getRangeAt(0);
  return `${selection.toString()}\u0000${range.startOffset}\u0000${range.endOffset}`;
}

function pointInsideRange(range: Range, x: number, y: number) {
  return [...range.getClientRects()].some((rect) => x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom);
}

/** Lets a sentence's own toolbar open translation without a second floating bubble. */
export const TranscriptTranslateContext = createContext<((segmentId: string, text: string, anchor: DOMRect) => void) | null>(null);

export function TranscriptAssist({ children, segments, sessionId, configured, onSave }: { children: ReactNode; segments: Segment[]; sessionId: string; configured: boolean; onSave: (segmentId: string, text: string, source: string) => Promise<boolean> }) {
  const root = useRef<HTMLDivElement>(null);
  const pointerStart = useRef<{ x: number; y: number; selection: string } | null>(null);
  const selectionButton = useRef<HTMLButtonElement>(null);
  const [selection, setSelection] = useState<SelectionAnchor | null>(null);
  const [dialog, setDialog] = useState<SelectionAnchor | null>(null);
  const [target, setTarget] = useState<'zh' | 'en'>('zh');
  const [ready, setReady] = useState(configured);
  const [result, setResult] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const saveLock = useRef(false);
  const request = useRef(0);
  const active = useRef(false);
  const close = () => { request.current += 1; active.current = false; saveLock.current = false; setSaving(false); setBusy(false); setDialog(null); setResult(''); setError(''); };
  useEffect(() => setReady(configured), [configured]);
  useEffect(() => { close(); setSelection(null); }, [sessionId]);
  useEffect(() => () => { request.current += 1; }, []);
  useEffect(() => {
    if (!selection) return;
    const dismiss = (event: MouseEvent) => { if (!(event.target as Element).closest('.selection-assist')) setSelection(null); };
    // Live following glides the transcript under the bubble; only a reader's own scroll dismisses it.
    const scroll = (event: Event) => { if (event.target instanceof Element && event.target.classList.contains('is-following')) return; setSelection(null); };
    document.addEventListener('mousedown', dismiss);
    window.addEventListener('scroll', scroll, true);
    return () => { document.removeEventListener('mousedown', dismiss); window.removeEventListener('scroll', scroll, true); };
  }, [selection]);
  const captureSelection = (event?: React.MouseEvent<HTMLDivElement>) => {
    const sel = window.getSelection();
    const rootNode = root.current;
    if (!sel || !rootNode) { setSelection(null); return; }
    if (sel.isCollapsed || !rootNode.contains(sel.anchorNode) || !rootNode.contains(sel.focusNode) || !sel.rangeCount) { setSelection(null); return; }
    const range = sel.getRangeAt(0);
    const value = transcriptSelection(rootNode, range);
    if (!value) { setSelection(null); return; }
    const rect = range.getBoundingClientRect();
    setSelection({ ...value, x: event?.clientX || rect.left, y: event?.clientY || rect.bottom });
  };
  const translate = async (value: SelectionAnchor, language: 'zh' | 'en') => {
    if (active.current) return;
    if (value.text.length > 12_000) { setError('请将选中文字缩短至 12,000 个字符以内。'); return; }
    const current = ++request.current; active.current = true; setBusy(true); setError(''); setResult('');
    try { const reply = await adapter.translateText(value.text, language); if (current === request.current) setResult(reply.text); }
    catch (cause) { if (current === request.current) setError(String(cause)); }
    finally { if (current === request.current) { active.current = false; setBusy(false); } }
  };
  const openAndTranslate = (value: SelectionAnchor) => {
    request.current += 1; active.current = false; setBusy(false); setSelection(null); setDialog(value); setResult(''); setError('');
    if (ready) void translate(value, target);
  };
  return <div ref={root} className="transcript-assist" onMouseDown={(event) => {
    if (event.button === 0) pointerStart.current = { x: event.clientX, y: event.clientY, selection: selectionSignature(window.getSelection()) };
  }} onMouseUp={(event) => {
    if (event.button !== 0) return;
    const start = pointerStart.current;
    pointerStart.current = null;
    const dragged = Boolean(start && Math.hypot(event.clientX - start.x, event.clientY - start.y) > 4);
    const changedSelection = Boolean(start && selectionSignature(window.getSelection()) && selectionSignature(window.getSelection()) !== start.selection);
    // A plain click selects the sentence, whose own toolbar carries 翻译; only a text
    // selection (drag, double-click) gets the floating translate bubble.
    if (dragged || changedSelection || event.detail > 1) captureSelection(event); else setSelection(null);
  }} onKeyUp={(event) => {
    if (event.key.startsWith('Arrow') && event.shiftKey) captureSelection();
  }} onContextMenu={(event) => {
    const selected = window.getSelection();
    const inside = selected && root.current?.contains(selected.anchorNode) && root.current.contains(selected.focusNode);
    const element = (event.target as Element).closest<HTMLElement>('[data-segment]');
    const sentence = (event.target as Element).closest<HTMLElement>('.segment-copy:not(.segment-ghost)');
    const segment = segments.find((item) => item.id === element?.dataset.segment);
    if (!segment) return;
    event.preventDefault();
    const useSelection = Boolean(inside && selected && !selected.isCollapsed && selected.rangeCount && pointInsideRange(selected.getRangeAt(0), event.clientX, event.clientY));
    const selectedValue = useSelection && root.current ? transcriptSelection(root.current, selected!.getRangeAt(0)) : null;
    setSelection({ text: selectedValue?.text ?? normalizedTranscriptText(sentence?.textContent ?? segment.displayText), segmentId: selectedValue?.segmentId ?? segment.id, x: event.clientX || element!.getBoundingClientRect().left, y: event.clientY || element!.getBoundingClientRect().bottom });
  }}><TranscriptTranslateContext.Provider value={(segmentId, text, anchor) => openAndTranslate({ segmentId, text: normalizedTranscriptText(text), x: anchor.left, y: anchor.bottom })}>{children}</TranscriptTranslateContext.Provider>
    {selection && createPortal(<div className="selection-assist" onMouseUp={(event) => event.stopPropagation()} onClick={(event) => event.stopPropagation()} style={{ left: Math.max(12, Math.min(selection.x, window.innerWidth - 112)), top: Math.max(12, Math.min(selection.y + 10, window.innerHeight - 58)) }}><button ref={selectionButton} type="button" aria-haspopup="dialog" onMouseDown={(event) => event.preventDefault()} onClick={() => openAndTranslate(selection)}><Languages size={15} /> 翻译</button></div>, document.body)}
    {dialog && <AssistDialog onClose={() => { if (!saveLock.current) close(); }}>
      <header><div><h2><Languages size={20} /> 译文</h2><p>{busy ? '正在翻译所选内容…' : '翻译完成后可保存为课堂备注。'}</p></div><button type="button" className="icon-button" aria-label="关闭翻译" disabled={saving} onClick={close}><X size={18} /></button></header>
      <blockquote className="translation-source">{dialog.text}</blockquote>
      {!ready && <DeepSeekConnection configured={configured} onConfigured={(value) => { setReady(value); if (value) void translate(dialog, target); }} />}
      <div className="translation-controls"><label>译为<select value={target} disabled={busy || saving || !ready} onChange={(event) => { const language = event.target.value as 'zh' | 'en'; setTarget(language); setResult(''); if (ready) void translate(dialog, language); }}><option value="zh">简体中文</option><option value="en">English</option></select></label>{busy && <span className="translation-progress" role="status"><LoaderCircle size={15} className="ai-spinner" /> 翻译中…</span>}</div>
      {ready && <p className="ai-disclosure">{dialog.text.length > 12000 ? '请将选中文字缩短至 12,000 个字符以内。' : '只发送上方正文到 DeepSeek。'}</p>}
      {error && <div className="translation-error" role="alert"><span><CircleAlert size={15} />{error}</span><button type="button" className="secondary-button" disabled={busy} onClick={() => void translate(dialog, target)}>重试</button></div>}
      {result && <><div className="translation-result" tabIndex={0} role="status" aria-live="polite">{result}</div><footer><button type="button" className="secondary-button" onClick={close} disabled={saving}>关闭</button><button type="button" className="primary-button" disabled={saving} onClick={async () => { if (saveLock.current) return; saveLock.current = true; const current = request.current; setSaving(true); try { const ok = await onSave(dialog.segmentId, `${dialog.text}\n\n${result}`, `DeepSeek · ${target === 'zh' ? '简体中文' : 'English'}`); if (current === request.current) { if (ok) close(); else setError('译文保存失败，请重试。'); } } catch (cause) { if (current === request.current) setError(String(cause)); } finally { if (current === request.current) { saveLock.current = false; setSaving(false); } } }}>{saving ? '保存中…' : '保存译文'}</button></footer></>}
    </AssistDialog>}
  </div>;
}

export function FormulaRecognitionPanel({ imageData, configured, disabled = false, onUse }: { imageData: string; configured: boolean; disabled?: boolean; onUse: (latex: string) => void }) {
  const [ready, setReady] = useState(configured);
  const [showConnection, setShowConnection] = useState(false);
  const [busy, setBusy] = useState(false);
  const [latex, setLatex] = useState('');
  const [error, setError] = useState('');
  const generation = useRef(0);
  const pending = useRef(false);
  useEffect(() => setReady(configured), [configured]);
  useEffect(() => { generation.current += 1; pending.current = false; setBusy(false); setLatex(''); setError(''); }, [imageData]);
  useEffect(() => () => { generation.current += 1; }, []);
  const recognize = async () => {
    if (!ready) { setShowConnection(true); return; }
    if (pending.current) return;
    const request = ++generation.current; pending.current = true; setBusy(true); setError('');
    try { const result = await adapter.recognizeFormula(imageData); if (request === generation.current) setLatex(result.latex); }
    catch (cause) { if (request === generation.current) setError(String(cause)); }
    finally { if (request === generation.current) { pending.current = false; setBusy(false); } }
  };
  let html = '';
  if (latex) { try { html = katex.renderToString(latex, { displayMode: true, throwOnError: true, trust: false, maxExpand: 1000 }); } catch { /* Raw output stays editable below. */ } }
  return <section className="formula-recognition" aria-label="图片公式识别">
    <div className="formula-action"><span><ScanText size={17} /><strong>图片转公式</strong></span><button type="button" className="secondary-button" disabled={disabled || busy || adapter.mode === 'demo'} onClick={() => void recognize()}>{busy ? <><LoaderCircle size={15} className="ai-spinner" /> 识别中…</> : '识别这张图片'}</button></div>
    <p className="ai-disclosure">发送这张图片到 DeepSeek，生成可编辑的 LaTeX 公式。</p>
    {showConnection && !ready && <DeepSeekConnection configured={configured} onConfigured={setReady} />}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {latex && <><label className="composer-field"><span>识别结果</span><textarea aria-label="识别出的公式" value={latex} disabled={disabled} onChange={(event) => setLatex(event.target.value)} /></label>{html ? <div className="formula-preview" dangerouslySetInnerHTML={{ __html: html }} /> : <p className="inline-error">公式格式需要调整，可直接编辑上方内容。</p>}<button type="button" className="secondary-button" disabled={disabled || !html} onClick={() => onUse(latex)}><Check size={15} /> 使用此公式</button></>}
  </section>;
}
