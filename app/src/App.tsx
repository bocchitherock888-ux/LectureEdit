import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  ArchiveRestore, BookOpen, Check, ChevronDown, ChevronLeft, ChevronRight, CircleAlert, Clock3, Cloud, Download, FileAudio, Folder, FolderPlus, HardDrive,
  FileText, Image as ImageIcon, Languages, Mic, MoreHorizontal, Pause, Pencil, Play, Plus, Quote,
  Redo2, Save, Search, Settings as SettingsIcon, Sigma, StickyNote, Undo2, Upload, X,
} from 'lucide-react';
import {
  FloatingFocusManager, FloatingPortal, autoUpdate, flip, offset, shift, size, useClick,
  useDismiss, useFloating, useInteractions, useRole,
} from '@floating-ui/react';
import { AnimatePresence, LayoutGroup, motion, useReducedMotion } from 'motion/react';
import { animate } from 'motion';
import katex from 'katex';
import 'katex/dist/katex.min.css';
import { adapter } from './bridge';
import { shortcutLabel, canExportNativePdf, isPrimaryShortcut, PDF_PLATFORM_NOTICE } from './platform';
import { FormulaRecognitionPanel, TranscriptAssist } from './DeepSeekTools';
import { GlobalSearch, type SearchResult } from './GlobalSearch';
import { modelStoppedRunning, SettingsDialog } from './SettingsDialog';
import { changedTranscriptSegments, persistTranscriptEdits, transcriptTextMap, TranscriptSaveError, type TranscriptTextMap } from './documentEditing';
import { sentencePlaybackSlices, type SentencePlaybackSlice } from './sentencePlayback';
import { followStep, nextFollowGoal, shouldPauseTranscriptFollow, transcriptFollowTarget } from './transcriptFollow';
import type { AppState, Draft, Note, NoteKind, Project, RuntimeInfo, Segment, Session } from './types';
import { commandId } from './types';

type SaveStatus = 'idle' | 'dirty' | 'saving' | 'saved' | 'restored' | 'error' | 'commitError';
type Modal = 'new' | 'settings' | 'review' | null;

function SessionMenu({ session, projects, open, onOpenChange, onMove, onRename }: {
  session: Session;
  projects: Project[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onMove: (projectId: string | null) => void;
  onRename: () => void;
}) {
  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange,
    placement: 'bottom-end',
    strategy: 'fixed',
    whileElementsMounted: autoUpdate,
    middleware: [offset(5), flip({ padding: 10 }), shift({ padding: 10 })],
  });
  const click = useClick(context);
  const dismiss = useDismiss(context);
  const role = useRole(context, { role: 'menu' });
  const { getReferenceProps, getFloatingProps } = useInteractions([click, dismiss, role]);

  return <>
    <button ref={refs.setReference} className="course-more" aria-label={`管理 ${session.title}`} aria-expanded={open} aria-haspopup="menu" {...getReferenceProps()}><MoreHorizontal size={15} /></button>
    {open && <FloatingPortal>
      <FloatingFocusManager context={context} modal={false} returnFocus>
        <div ref={refs.setFloating} style={floatingStyles} className="course-context-menu glass-surface" {...getFloatingProps()}>
          <button role="menuitem" onClick={() => { onRename(); onOpenChange(false); }}><Pencil size={13} /> 重命名课堂</button>
          <strong>移到课程分组</strong>
          <button role="menuitem" className={session.projectId === null ? 'selected' : ''} onClick={() => { onMove(null); onOpenChange(false); }}>未分组</button>
          {projects.map(project => <button role="menuitem" title={project.title} className={session.projectId === project.id ? 'selected' : ''} key={project.id} onClick={() => { onMove(project.id); onOpenChange(false); }}>{project.title}</button>)}
        </div>
      </FloatingFocusManager>
    </FloatingPortal>}
  </>;
}

const fmtTime = (samples: number) => {
  const total = Math.max(0, Math.round(samples / 16000));
  const minutes = Math.floor(total / 60);
  return `${String(minutes).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
};
/** Paragraph start on the course clock: run offset plus the position inside that run. */
const segmentClock = (session: Session, segment: Segment) => (session.runs.find((run) => run.id === segment.runId)?.offsetMs ?? 0) * 16 + segment.startSample;
/** Chinese characters count one each; Latin text counts by words. */
const countWords = (segments: Segment[]) => segments.reduce((sum, segment) => {
  const text = segment.displayText;
  return sum + (text.match(/[\u3400-\u9fff]/g)?.length ?? 0) + (text.replace(/[\u3400-\u9fff]/g, ' ').match(/[A-Za-z0-9'’-]+/g)?.length ?? 0);
}, 0);
const fmtDuration = (samples: number) => {
  const total = Math.max(0, Math.floor(samples / 16000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return hours > 0 ? `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}` : `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`;
};
const fmtDate = (value: number) => new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' }).format(value);
const recordingLabel: Record<string, string> = { idle: '可以开始录音', starting: '正在启动麦克风', recording: '录音中', paused: '录音已暂停', stopping: '正在结束录音', stopped: '录音已结束', error: '录音发生错误' };
const inferenceLabel: Record<string, string> = { idle: '等待转写', unloaded: '本地模型未加载', loading: '正在加载本地模型', ready: '本地模型可用', running: '正在本地转写', catching_up: '正在补齐转写', error: '转写发生错误' };

function IconButton({ label, children, onClick, disabled, active, expanded }: { label: string; children: React.ReactNode; onClick?: () => void; disabled?: boolean; active?: boolean; expanded?: boolean }) {
  return <button className={`icon-button ${active ? 'is-active' : ''}`} aria-label={label} aria-haspopup={expanded === undefined ? undefined : 'dialog'} aria-expanded={expanded} title={label} onClick={onClick} disabled={disabled}>{children}</button>;
}

function moveMenuFocus(event: React.KeyboardEvent<HTMLElement>) {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])')).filter((item) => item.getClientRects().length > 0);
  if (!items.length) return;
  event.preventDefault();
  const current = items.indexOf(document.activeElement as HTMLElement);
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : event.key === 'ArrowDown' ? (current + 1 + items.length) % items.length : (current - 1 + items.length) % items.length;
  items[next]?.focus();
}

function StatusDot({ state }: { state: string }) {
  return <span className={`status-dot status-${state}`} aria-hidden="true" />;
}

function GlassSurface({ className = '', children }: { className?: string; children: React.ReactNode }) {
  return <div className={`glass-surface ${className}`}>{children}</div>;
}

function RecordGlyph({ state }: { state: 'ready' | 'starting' | 'recording' | 'paused' | 'stopping' | 'blocked' }) {
  return <span className={`record-glyph is-${state}`} aria-hidden="true"><span /></span>;
}

/** Decorative activity bars: they signal a live capture, not a measured level. */
function LevelMeter({ live }: { live: boolean }) {
  return <span className={`level-meter ${live ? 'is-live' : ''}`} aria-hidden="true"><i /><i /><i /><i /><i /></span>;
}

function EmptyTranscriptState({ session, samples, demo, cloud, cloudReady, legacyEngine, retryDisabled, runtime, onStart, onImport, onRetry, onPrepare, onSetup }: { session: Session; samples: number; demo: boolean; cloud: boolean; cloudReady: boolean; legacyEngine: boolean; retryDisabled: boolean; runtime: RuntimeInfo; onStart: () => void; onImport: () => void; onRetry: () => void; onPrepare: () => void; onSetup: () => void }) {
  const captureActive = ['starting', 'recording', 'paused', 'stopping'].includes(session.recordingState);
  const processing = ['loading', 'running', 'catching_up'].includes(session.inferenceState);
  const hasAudio = samples > 0 || session.runs.length > 0;
  if (captureActive) {
    const heading = session.recordingState === 'starting' ? '正在准备录音…' : session.recordingState === 'paused' ? '录音已暂停' : session.recordingState === 'stopping' ? '正在完成录音…' : '正在录音';
    const status = cloud ? session.inferenceState === 'paused' ? '云端转写已暂停，录音继续保存在本机' : '正在连接云端实时转写' : inferenceLabel[session.inferenceState] || '正在准备本地转写';
    return <div className="empty-state capture-active"><div className={`capture-symbol is-${session.recordingState}`}><span /></div><h2>{heading}</h2><p>{status}。识别出的内容会自动出现在这里。</p><div className="capture-progress" role="status" aria-label={`已录制 ${fmtDuration(samples)}`}><strong>{fmtDuration(samples)}</strong><LevelMeter live={session.recordingState === 'recording'} /><span>录音已保存在本机</span></div></div>;
  }
  const installed = runtime.modelInstalled ?? runtime.modelReady ?? false;
  const modelState = runtime.modelState ?? (runtime.modelReady ? 'ready' : 'unloaded');
  if (!demo && legacyEngine) return <div className="empty-state model-waiting"><SettingsIcon size={31} /><h2>请选择实时转写引擎</h2><p>当前课程保留全部录音。切换到 Qwen 本地识别或 Soniox 云端实时识别后即可继续转写。</p>{hasAudio && <div className="capture-progress"><strong>{fmtDuration(samples)}</strong><span>已保存到本机</span></div>}<button className="secondary-button" onClick={onSetup}>打开转写设置</button></div>;
  if (!demo && cloud && !cloudReady) return <div className="empty-state model-waiting"><Cloud size={31} /><h2>需要配置云端转写</h2><p>{hasAudio ? '录音已安全保存。' : ''}在设置中填写 Soniox API Key 并确认云端发送音频后，即可开始录音与转写。</p>{hasAudio && <div className="capture-progress"><strong>{fmtDuration(samples)}</strong><span>已保存到本机</span></div>}<button className="secondary-button" onClick={onSetup}>打开云端转写设置</button></div>;
  if (!demo && !cloud && !runtime.modelReady) {
    const loading = modelState === 'loading';
    const failed = modelState === 'error';
    return <div className="empty-state model-waiting"><SettingsIcon size={31} /><h2>{loading ? '正在准备本地模型' : failed ? modelStoppedRunning(runtime.modelError) ? '本地模型已停止运行' : '本地模型准备失败' : installed ? '本地模型尚未准备' : '需要本地转写模型'}</h2><p>{failed && runtime.modelError ? runtime.modelError : loading ? '模型加载完成后即可开始录音。' : hasAudio ? '录音已安全保存。准备好模型后可以继续转写。' : installed ? '模型文件已安装，完成加载后即可开始录音。' : '下载并校验模型后，音频会留在这台电脑上完成转写。'}</p>{hasAudio && <div className="capture-progress"><strong>{fmtDuration(samples)}</strong><span>已保存到本机</span></div>}{!loading && <div>{installed && <button className="primary-button" onClick={onPrepare}>{failed ? '重试准备模型' : '准备本地模型'}</button>}<button className="secondary-button" onClick={onSetup}>{installed ? '打开模型设置' : '下载本地模型'}</button></div>}</div>;
  }
  if (hasAudio && processing) return <div className="empty-state processing-state"><Clock3 size={31} /><h2>录音已结束，正在处理</h2><p>{cloud ? '云端正在完成转写' : inferenceLabel[session.inferenceState]}。新的转写块会自动显示在这里。</p><div className="capture-progress"><strong>{fmtDuration(samples)}</strong><span>录音已保存在本机</span></div></div>;
  if (hasAudio) return <div className="empty-state"><FileAudio size={31} /><h2>录音已结束</h2><p>当前录音还没有生成可显示的转写。音频已经保存在本机，可以重新尝试转写。</p><div className="capture-progress"><strong>{fmtDuration(samples)}</strong><span>录音已保存在本机</span></div><button className="secondary-button" disabled={retryDisabled} onClick={onRetry}><ArchiveRestore size={16} /> {retryDisabled ? '另一课程正在转写' : '重试转写'}</button></div>;
  return <div className="empty-state idle-state"><p className="empty-eyebrow">{session.title}</p><button className="record-hero" onClick={onStart} aria-label={demo ? '预览录音状态' : '开始录音'}><span /></button><h2>等待课程开始</h2><p>点击红色按钮开始录音，或导入已有的 WAV 文件。转写进行时，你可以随时修订已出现的内容。</p><div><button className="secondary-button" onClick={onImport}><FileAudio size={16} /> 导入 WAV</button></div></div>;
}

function Formula({ latex }: { latex: string }) {
  try {
    return <div className="formula-preview" dangerouslySetInnerHTML={{ __html: katex.renderToString(latex, { displayMode: true, throwOnError: true, trust: false, strict: 'warn' }) }} />;
  } catch {
    return <div className="formula-error"><CircleAlert size={15} /> 公式无法渲染，LaTeX 源码已保留。</div>;
  }
}

function NoteBlock({ note, onRemove }: { note: Note; onRemove?: () => void }) {
  const [confirming, setConfirming] = useState(false);
  useEffect(() => { if (!confirming) return; const timer = window.setTimeout(() => setConfirming(false), 4000); return () => clearTimeout(timer); }, [confirming]);
  const meta: [string, typeof StickyNote] = note.kind === 'example' ? ['例子', Quote] : note.kind === 'formula' ? ['公式', Sigma] : note.kind === 'image' ? ['图片', ImageIcon] : ['备注', StickyNote];
  const Mark = meta[1];
  return <section className={`note-block note-${note.kind}`} data-note={note.id}>
    <header><span><Mark size={14} /> {meta[0]}</span>{note.sourceLabel && <span className="note-source">{note.sourceLabel}</span>}{onRemove && (confirming ? <button className="quiet-danger note-remove-confirm" autoFocus onBlur={() => setConfirming(false)} onClick={() => { setConfirming(false); onRemove(); }}>再点一次确认删除</button> : <IconButton label="移除课堂资料" onClick={() => setConfirming(true)}><X size={14} /></IconButton>)}</header>
    {note.kind === 'formula' ? <Formula latex={note.text} /> : note.kind === 'image' && note.imageData ? <><img src={note.imageData} alt={note.text || '课堂图片'} />{note.text && <p className="caption">{note.text}</p>}</> : <p>{note.text}</p>}
  </section>;
}

type TranscriptGroupingState = { known: Set<string>; paragraphStarts: Set<string> };

export function groupTranscriptSegments(segments: Segment[], stable?: TranscriptGroupingState) {
  if (stable) {
    const live = new Set(segments.map((segment) => segment.id));
    stable.known = new Set([...stable.known].filter((id) => live.has(id)));
    stable.paragraphStarts = new Set([...stable.paragraphStarts].filter((id) => live.has(id)));
  }
  const groups: Segment[][] = [];
  for (const segment of segments) {
    const current = groups.at(-1);
    const length = current?.reduce((sum, item) => sum + item.displayText.length, 0) ?? 0;
    const previous = current?.at(-1);
    const naturalBreak = Boolean(previous && /[.!?。！？][”’」』]?\s*$/.test(previous.displayText));
    const newRun = Boolean(previous && previous.runId !== segment.runId);
    const newSegment = stable ? !stable.known.has(segment.id) : false;
    const shouldStart = !current || newRun || (stable?.paragraphStarts.has(segment.id) ?? false) || ((stable ? newSegment : true) && (length >= 360 || (length >= 220 && naturalBreak)));
    if (shouldStart) {
      groups.push([segment]);
      stable?.paragraphStarts.add(segment.id);
    }
    else current.push(segment);
    stable?.known.add(segment.id);
  }
  return groups;
}

function streamingPieces(text: string) {
  if (!text) return [];
  const segmenter = new Intl.Segmenter(undefined, { granularity: 'word' });
  const pieces: string[] = [];
  let pendingSpace = '';
  for (const part of segmenter.segment(text)) {
    if (/^\s+$/.test(part.segment)) pendingSpace += part.segment;
    else if (part.isWordLike) { pieces.push(pendingSpace + part.segment); pendingSpace = ''; }
    else if (pieces.length > 0) pieces[pieces.length - 1] += pendingSpace + part.segment;
    else { pendingSpace += part.segment; }
  }
  if (pendingSpace) {
    if (pieces.length > 0) pieces[pieces.length - 1] += pendingSpace;
    else pieces.push(pendingSpace);
  }
  return pieces;
}

function TranscriptBlock({ segment, active, playingKey, entering, query, composer, onEdit, onPlay, onUndo, onRedo, onReview }: {
  segment: Segment; active: boolean; playingKey: string | null; entering: boolean; query: string; composer: (sliceKey: string) => React.ReactNode; onEdit: () => void; onPlay: (anchor: NonNullable<SearchResult['audioAnchor']>, playbackKey: string) => void; onUndo: () => void; onRedo: () => void; onReview: () => void;
}) {
  const text = segment.displayText;
  const reduceMotion = useReducedMotion();
  const previousState = useRef({ text, machineRevision: segment.machineRevision });
  const previous = previousState.current;
  const machineGrew = segment.machineRevision > previous.machineRevision && text.length > previous.text.length && text.startsWith(previous.text);
  const appendedSuffix = !query.trim() && machineGrew ? text.slice(previous.text.length) : '';
  useEffect(() => { previousState.current = { text, machineRevision: segment.machineRevision }; }, [segment.machineRevision, text]);
  const slices = useMemo(() => sentencePlaybackSlices(text, segment.startSample, segment.endSample), [segment.endSample, segment.startSample, text]);
  const renderSlice = (slice: SentencePlaybackSlice) => {
    if (appendedSuffix && slice.endOffset > previous.text.length) {
      const boundary = Math.max(slice.startOffset, previous.text.length);
      const pieces = streamingPieces(text.slice(boundary, slice.endOffset));
      // Whitespace stays outside the inline-block tokens; inside them it collapses and words run together mid-animation.
      return <>{text.slice(slice.startOffset, boundary)}<span key={`${segment.machineRevision}-${boundary}`} className="stream-suffix">{pieces.map((piece, index) => {
        const [, lead, word, trail] = /^(\s*)([\s\S]*?)(\s*)$/.exec(piece) ?? ['', '', piece, ''];
        return <Fragment key={`${boundary + index}:${piece}`}>{lead}{word && <span className={`stream-token ${index === pieces.length - 1 ? 'is-last' : ''}`} style={{ animationDelay: `${Math.min(index, 10) * 22}ms` }}>{word}</span>}{trail}</Fragment>;
      })}</span></>;
    }
    if (!query.trim()) return slice.text;
    const localIndex = slice.text.toLocaleLowerCase().indexOf(query.toLocaleLowerCase());
    if (localIndex < 0) return slice.text;
    return <>{slice.text.slice(0, localIndex)}<mark>{slice.text.slice(localIndex, localIndex + query.length)}</mark>{slice.text.slice(localIndex + query.length)}</>;
  };
  return <motion.span
    className={`transcript-block ${active ? 'is-editing' : ''} ${segment.final ? 'is-final' : 'is-live'}`}
    data-segment={segment.id}
    initial={entering && !reduceMotion ? { opacity: 0, y: 3, filter: 'blur(1.5px)' } : false}
    animate={{ opacity: 1, y: 0, filter: 'blur(0px)' }}
    transition={{ type: 'spring', bounce: 0, duration: reduceMotion ? 0 : .42 }}
  >
    {active ? <span className="segment-copy segment-ghost" aria-hidden="true">{text}</span> : slices.map((slice, index) => {
      const sliceKey = `${segment.id}:${index}`;
      const playing = playingKey === sliceKey;
      const anchor = { runId: segment.runId, startSample: slice.startSample, endSample: slice.endSample };
      return <span className="sentence-unit" key={sliceKey}>
        <span className={`segment-copy ${playing ? 'is-playing' : ''}`} role="button" tabIndex={0} aria-label={`选择并操作这句：${slice.text.trim()}`} title="选择这句，显示回放与修订操作" onClick={(event) => { if (window.getSelection()?.isCollapsed !== false) event.currentTarget.focus(); }} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.currentTarget.focus(); } }}>{renderSlice(slice)}</span>
        <span className="segment-tools" onClick={(event) => event.stopPropagation()}>
          <button onClick={() => onPlay(anchor, sliceKey)} aria-label={playing ? '暂停这一句的录音' : `回放 ${fmtTime(slice.startSample)} 开始的这一句`}>{playing ? <Pause size={13} fill="currentColor" /> : <Play size={13} fill="currentColor" />} {playing ? '暂停' : '回放这句'}</button>
          <button onClick={onEdit} title="打开这句所在转写段"><Pencil size={13} /> 取出编辑</button>
          {segment.history.length > 0 && segment.historyIndex >= 0 && <button onClick={onUndo} aria-label="撤销此句最近一次修订" title="撤销此句最近一次修订"><Undo2 size={13} /> 撤销修订</button>}
          {segment.history.length > 0 && segment.historyIndex < segment.history.length - 1 && <button onClick={onRedo} aria-label="重做此句修订" title="重做此句修订"><Redo2 size={13} /> 重做修订</button>}
          {composer(sliceKey)}
          {segment.pendingMachine && <button onClick={onReview}><CircleAlert size={13} /> 核对更新</button>}
        </span>
      </span>;
    })}
  </motion.span>;
}

function EditableTranscriptSegment({ segment, text, disabled, onText }: { segment: Segment; text: string; disabled: boolean; onText: (text: string) => void }) {
  const node = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    if (node.current && document.activeElement !== node.current && node.current.textContent !== text) node.current.textContent = text;
  }, [segment.id, text]);
  return <span
    ref={node}
    className="full-edit-segment"
    contentEditable={!disabled}
    suppressContentEditableWarning
    role="textbox"
    aria-multiline="false"
    aria-disabled={disabled}
    aria-label={`编辑 ${fmtTime(segment.startSample)} 的转写`}
    title={`${fmtTime(segment.startSample)} · 点击编辑`}
    data-segment={segment.id}
    onInput={(event) => onText(event.currentTarget.textContent ?? '')}
    onKeyDown={(event) => { if (event.key === 'Enter') event.preventDefault(); }}
    onPaste={(event) => { event.preventDefault(); document.execCommand('insertText', false, event.clipboardData.getData('text/plain').replace(/[\r\n]+/g, ' ')); }}
  />;
}

function FullTranscriptEditor({ session, values, original, status, error, onText, onSave, onDone }: {
  session: Session;
  values: TranscriptTextMap;
  original: TranscriptTextMap;
  status: 'editing' | 'saving' | 'saved' | 'error';
  error: string;
  onText: (segmentId: string, text: string) => void;
  onSave: () => void;
  onDone: () => void;
}) {
  const changed = changedTranscriptSegments(session.segments, original, values).length;
  const statusText = status === 'saving' ? '正在写入本机…' : status === 'error' ? '尚有修改未保存' : changed ? `${changed} 句待保存` : '全部修改已保存到本机';
  return <div className="full-editor" aria-label="全文编辑器">
    <div className="full-edit-bar glass-surface">
      <div><Pencil size={15} /><span><strong>编辑全文</strong><small>{statusText}</small></span></div>
      <div className="full-edit-actions"><button className="secondary-button" onClick={onDone} disabled={status === 'saving'}>完成</button><button className="primary-button" onClick={onSave} disabled={!changed || status === 'saving'}><Save size={14} />{status === 'saving' ? '保存中…' : '保存修改'}</button></div>
    </div>
    <div className="session-intro full-edit-intro"><div><div className="session-title-row"><h2>{session.title}</h2><span className="editing-label">编辑中</span></div><p className="session-meta"><span>{new Intl.DateTimeFormat('zh-CN', { weekday: 'long', month: 'long', day: 'numeric' }).format(session.createdAt)}</span><span>每句话保留原录音时间点</span></p></div></div>
    {error && <p className="full-edit-error" role="alert"><CircleAlert size={15} />{error}</p>}
    <div className="full-edit-copy" spellCheck>{groupTranscriptSegments(session.segments).map((group) => <section className="transcript-paragraph full-edit-paragraph" key={group[0].id}><p>{group.map((segment, index) => <Fragment key={segment.id}>{index > 0 ? ' ' : null}<EditableTranscriptSegment segment={segment} text={values[segment.id] ?? ''} disabled={status === 'saving'} onText={(text) => onText(segment.id, text)} /></Fragment>)}</p>{group.flatMap((segment) => session.notes.filter((note) => note.segmentId === segment.id)).map((note) => <NoteBlock key={note.id} note={note} />)}</section>)}</div>
    <div className="full-edit-finish"><span>{changed ? `${changed} 句有修改` : '文稿已保存'}</span><button className="primary-button" onClick={onDone} disabled={status === 'saving'}>{changed ? '保存并完成' : '完成编辑'}</button></div>
  </div>;
}

function CourseSidebar({ projects, sessions, selectedId, expanded, revealProjectId, onToggle, onSearch, onSelect, onNew, onNewProjectCourse, onCreateProject, onRenameProject, onMove, onRename, onImport }: {
  projects: Project[]; sessions: Session[]; selectedId: string | null; expanded: boolean; revealProjectId: string | null; onToggle: () => void; onSearch: () => void; onSelect: (id: string) => void; onNew: () => void; onNewProjectCourse: (projectId: string) => void; onCreateProject: (title: string) => Promise<Project | null>; onRenameProject: (projectId: string, title: string) => void; onMove: (sessionId: string, projectId: string | null) => void; onRename: (id: string, title: string) => void; onImport: () => void;
}) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renamingProject, setRenamingProject] = useState<string | null>(null);
  const [creatingProject, setCreatingProject] = useState(false);
  const [projectTitle, setProjectTitle] = useState('');
  const [menu, setMenu] = useState<string | null>(null);
  const [openProjects, setOpenProjects] = useState<Set<string>>(() => {
    try { return new Set(JSON.parse(localStorage.getItem('lectureedit.open-projects') || '[]') as string[]); }
    catch { return new Set(); }
  });
  useEffect(() => { try { localStorage.setItem('lectureedit.open-projects', JSON.stringify([...openProjects])); } catch { /* Storage can be unavailable in restricted webviews. */ } }, [openProjects]);
  useEffect(() => { if (!expanded) setMenu(null); }, [expanded]);
  useEffect(() => { if (revealProjectId) setOpenProjects(current => new Set(current).add(revealProjectId)); }, [revealProjectId]);
  const toggleProject = (projectId: string) => setOpenProjects(current => { const next = new Set(current); if (next.has(projectId)) next.delete(projectId); else next.add(projectId); return next; });
  const [submittingProject, setSubmittingProject] = useState(false); const submittingProjectRef = useRef(false);
  const submitProject = async () => {
    const title = projectTitle.trim();
    if (!title || submittingProjectRef.current) return;
    submittingProjectRef.current = true; setSubmittingProject(true);
    const project = await onCreateProject(title).finally(() => { submittingProjectRef.current = false; setSubmittingProject(false); });
    if (!project) return;
    setOpenProjects(current => new Set(current).add(project.id));
    setProjectTitle(''); setCreatingProject(false);
  };
  const row = (session: Session) => <div className={`course-item ${session.id === selectedId ? 'selected' : ''}`} key={session.id}>
    {renaming === session.id && expanded ? <input autoFocus defaultValue={session.title} aria-label="课堂名称" onBlur={(event) => { onRename(session.id, event.target.value); setRenaming(null); }} onKeyDown={(event) => { if (event.nativeEvent.isComposing) return; if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') setRenaming(null); }} /> : <>
      <button className="course-select" aria-label={session.title} onClick={() => onSelect(session.id)} onDoubleClick={() => { if (expanded) setRenaming(session.id); }} title={session.title}>
        <BookOpen size={16} />{expanded && <span><strong>{session.title}</strong><small>{fmtDate(session.createdAt)}</small></span>}
      </button>
      {expanded && <SessionMenu session={session} projects={projects} open={menu === session.id} onOpenChange={(open) => setMenu(open ? session.id : null)} onRename={() => setRenaming(session.id)} onMove={(projectId) => onMove(session.id, projectId)} />}
    </>}
  </div>;
  const loose = sessions.filter(session => !session.projectId);
  return <aside className="course-sidebar glass-surface" aria-label="课程与课堂">
    <div className="app-mark"><div className="app-identity"><img src="/app-icon.png" alt="" />{expanded && <span>LectureEdit</span>}</div><button className="sidebar-toggle" aria-label={expanded ? '收起课堂栏' : '展开课堂栏'} aria-expanded={expanded} title={expanded ? '收起课堂栏' : '展开课堂栏'} onClick={onToggle}>{expanded ? <ChevronLeft className="direction-icon" size={16} strokeWidth={1.9} /> : <ChevronRight className="direction-icon" size={16} strokeWidth={1.9} />}</button></div>
    <button className="sidebar-search" aria-label="搜索所有课堂" title="搜索所有课堂" onClick={onSearch}><Search size={16} />{expanded && <span>搜索所有课堂</span>}{expanded && <kbd>{shortcutLabel('K')}</kbd>}</button>
    <nav>
      {expanded && <div className="sidebar-section-heading"><span>课堂记录</span><button aria-label="新建课程分组" title="新建课程分组" onClick={() => setCreatingProject(true)}><FolderPlus size={15} /></button></div>}
      {creatingProject && expanded && <div className="project-create"><Folder size={15} /><input autoFocus value={projectTitle} aria-label="课程分组名称" placeholder="例如：Economics" onChange={event => setProjectTitle(event.target.value)} onBlur={(event) => { if (!projectTitle.trim() && !event.currentTarget.parentElement?.contains(event.relatedTarget as Node | null)) setCreatingProject(false); }} onKeyDown={event => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) void submitProject(); if (event.key === 'Escape') { setProjectTitle(''); setCreatingProject(false); } }} /><button aria-label="创建课程分组" title="创建课程分组" disabled={!projectTitle.trim() || submittingProject} onClick={() => void submitProject()}><Check size={14} /></button></div>}
      {loose.map(row)}
      {projects.map(project => {
        const children = sessions.filter(session => session.projectId === project.id);
        const open = openProjects.has(project.id);
        if (!expanded) return children.map(row);
        return <section className="project-group" key={project.id}>
          <div className="project-heading">{renamingProject === project.id ? <input autoFocus defaultValue={project.title} aria-label="课程分组名称" onBlur={event => { onRenameProject(project.id, event.target.value); setRenamingProject(null); }} onKeyDown={event => { if (event.nativeEvent.isComposing) return; if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') setRenamingProject(null); }} /> : <button className="project-toggle" title={project.title} onClick={() => toggleProject(project.id)} onDoubleClick={() => setRenamingProject(project.id)} aria-expanded={open}><ChevronRight className={`disclosure-chevron ${open ? 'is-open' : ''}`} size={14} strokeWidth={1.9} /><Folder size={15} /><span>{project.title}</span><small>{children.length}</small></button>}{renamingProject !== project.id && <button className="project-rename" aria-label={`重命名 ${project.title}`} title="重命名课程分组" onClick={() => setRenamingProject(project.id)}><Pencil size={13} /></button>}<button className="project-add" aria-label={`在 ${project.title} 新建课堂`} title="在课程分组中新建课堂" onClick={() => { if (!open) toggleProject(project.id); onNewProjectCourse(project.id); }}><Plus size={14} /></button></div>
          {open && <div className="project-courses">{children.length ? children.map(row) : <button className="empty-project" onClick={() => onNewProjectCourse(project.id)}>新建第一节课</button>}</div>}
        </section>;
      })}
    </nav>
    <div className="sidebar-bottom">
      <button className="sidebar-action" aria-label="导入课程包" title="导入课程包" onClick={onImport}><Upload size={16} />{expanded && <span>导入</span>}</button>
      <button className="new-course" aria-label="新建课堂" title="新建课堂" onClick={onNew}><Plus size={16} />{expanded && <span>新建课堂</span>}</button>
    </div>
  </aside>;
}

type SegmentRect = { left: number; top: number; width: number; height: number };

const PANEL_SLIDE = 16;
const FLIGHT_EASE = [0.32, 0.72, 0, 1] as const;

function CorrectionPanel({ draft, segment, text, status, commitError, originRect, exiting, onText, onClose, onDiscard, onCommit, onUndo, onRedo, onPlay, onReview }: {
  draft: Draft; segment: Segment; text: string; status: SaveStatus; commitError: string; originRect?: SegmentRect; exiting?: boolean; onText: (text: string) => void; onClose: () => void; onDiscard: () => void; onCommit: () => void; onUndo: () => void; onRedo: () => void; onPlay: () => void; onReview: () => void;
}) {
  const composing = useRef(false);
  const reduceMotion = useReducedMotion();
  const textRef = useRef<HTMLTextAreaElement>(null);
  const [targetRect, setTargetRect] = useState<SegmentRect | null>(null);
  const [flightDone, setFlightDone] = useState(!originRect || Boolean(reduceMotion));
  // Grow the field with its text so a one-line sentence is not an empty slab; long text scrolls inside.
  const fitField = useCallback(() => { const field = textRef.current; if (!field) return; field.style.height = 'auto'; field.style.height = `${field.scrollHeight}px`; }, []);
  useLayoutEffect(fitField, [fitField, text]);
  useEffect(() => { window.addEventListener('resize', fitField); return () => window.removeEventListener('resize', fitField); }, [fitField]);
  useLayoutEffect(() => {
    const rect = textRef.current?.getBoundingClientRect();
    // The panel is still at its slide-in offset when this is measured.
    if (rect) setTargetRect({ left: rect.left + (reduceMotion ? 0 : PANEL_SLIDE), top: rect.top, width: rect.width, height: rect.height });
  }, [reduceMotion]);
  useEffect(() => { if (!originRect || reduceMotion || exiting) return; const timer = window.setTimeout(() => setFlightDone(true), 440); return () => clearTimeout(timer); }, [exiting, originRect, reduceMotion]);
  const origin = originRect ? { left: originRect.left, top: originRect.top, width: Math.min(originRect.width, 560), height: Math.min(originRect.height, 180) } : null;
  // A ring drawn with box-shadow, not a border, so the text box matches the textarea exactly and wraps identically on landing.
  const lifted = { boxShadow: '0 0 0 1px rgba(50,101,214,.2), 0 16px 36px rgba(24,34,48,.18), 0 2px 6px rgba(24,34,48,.08)' };
  const landed = { boxShadow: '0 0 0 1px rgba(50,101,214,0), 0 0 0 rgba(24,34,48,0), 0 0 0 rgba(24,34,48,0)' };
  const flight = origin && targetRect && !reduceMotion && (!flightDone || exiting) ? <FloatingPortal><motion.div className="extraction-flight" aria-hidden="true"
    initial={exiting ? { ...targetRect, ...landed, opacity: 1 } : { ...origin, ...lifted, opacity: 1 }}
    animate={exiting ? { ...origin, ...lifted, opacity: [1, 1, 0] } : { ...targetRect, ...landed, opacity: 1 }}
    transition={{ duration: .4, ease: FLIGHT_EASE, opacity: exiting ? { duration: .4, times: [0, .7, 1] } : undefined }}>{text}</motion.div></FloatingPortal> : null;
  const statusView = status === 'saving' ? '正在保存…' : status === 'saved' ? <><Check size={13} /> 草稿已保存</> : status === 'restored' ? <><ArchiveRestore size={13} /> 已恢复草稿</> : status === 'error' ? <><CircleAlert size={13} /> 草稿保存失败</> : status === 'commitError' ? <><CircleAlert size={13} /> 修订未保存：{commitError || '请重试'}</> : status === 'dirty' ? '未保存' : '自动保存';
  return <><motion.aside className={`correction-panel glass-surface ${exiting ? 'is-exiting' : ''}`} aria-label="修订编辑器"
    initial={reduceMotion ? { opacity: 0 } : { opacity: 0, x: -PANEL_SLIDE }}
    animate={exiting ? { opacity: 0, x: reduceMotion ? 0 : -PANEL_SLIDE } : { opacity: 1, x: 0 }}
    exit={{ opacity: 0 }}
    transition={{ duration: reduceMotion ? .12 : exiting ? .3 : .34, ease: FLIGHT_EASE }}>
    <header className="panel-header"><div><h2>修订 <span>{fmtTime(segment.startSample)}</span></h2><p>{segment.userSeq > 0 ? `第 ${segment.userSeq + 1} 次修订 · ` : ''}录音与转写照常进行</p></div><IconButton label="关闭并保留草稿" onClick={onClose}><X size={18} /></IconButton></header>
    <div className="editor-source"><button onClick={onPlay}><Play size={12} fill="currentColor" /> 回放此段</button><details><summary>原识别</summary><p>{draft.baseMachineText}</p></details></div>
    {segment.pendingMachine && <button className="editor-update" onClick={onReview}><CircleAlert size={15} /> 此段有新的识别结果待核对</button>}
    <div className={`editor-card ${status === 'error' || status === 'commitError' ? 'has-error' : ''}`}>
      <label className="sr-only" htmlFor="correction-text">修订内容</label>
      <textarea ref={textRef} id="correction-text" rows={1} autoFocus spellCheck={false} value={text} style={{ opacity: flightDone && !exiting ? 1 : 0 }} onChange={(e) => onText(e.target.value)} onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }} onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && !composing.current && !e.nativeEvent.isComposing) { e.preventDefault(); onCommit(); }
        if (e.key === 'Escape' && !composing.current) onClose();
      }} />
      <div className="editor-card-bar">
        <IconButton label="撤销" onClick={onUndo}><Undo2 size={15} /></IconButton><IconButton label="重做" onClick={onRedo}><Redo2 size={15} /></IconButton>
        <span className={`draft-status status-${status}`} role="status">{statusView}</span>
      </div>
    </div>
    <div className="editor-footer"><button className="quiet-danger" onClick={onDiscard}>放弃草稿</button><button className="primary-button" onClick={onCommit} disabled={status === 'saving'}>保存修订 <kbd>{shortcutLabel('Enter')}</kbd></button></div>
  </motion.aside>{flight}</>;
}

function Dialog({ title, description, children, onClose, wide = false }: { title: string; description?: string; children: React.ReactNode; onClose: () => void; wide?: boolean }) {
  useEffect(() => { const close = (event: KeyboardEvent) => { if (event.key === 'Escape' && !event.isComposing) onClose(); }; window.addEventListener('keydown', close); return () => window.removeEventListener('keydown', close); }, [onClose]);
  return <div className="dialog-layer" role="presentation" onMouseDown={(e) => { if (e.currentTarget === e.target) onClose(); }}><section className={`dialog ${wide ? 'dialog-wide' : ''}`} role="dialog" aria-modal="true" aria-label={title}>
    <header><div><h2>{title}</h2>{description && <p>{description}</p>}</div><IconButton label="关闭" onClick={onClose}><X size={18} /></IconButton></header>{children}
  </section></div>;
}

function InlineNoteComposer({ open, onOpenChange, kind, setKind, configured, onSubmit }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  kind: NoteKind;
  setKind: (kind: NoteKind) => void;
  configured: boolean;
  onSubmit: (value: { kind: NoteKind; text: string; sourceLabel: string; imageData: string | null }) => Promise<boolean>;
}) {
  const [text, setText] = useState('');
  const [source, setSource] = useState(kind === 'example' ? '教师例子' : '我的备注');
  const [image, setImage] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const composing = useRef(false);
  const reduceMotion = useReducedMotion();
  const workspaceBoundary = typeof document === 'undefined' ? undefined : document.querySelector<HTMLElement>('.workspace') ?? undefined;
  const guardedOpenChange = (next: boolean) => { if (!next && savingRef.current) return; onOpenChange(next); };
  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: guardedOpenChange,
    placement: 'bottom-start',
    strategy: 'fixed',
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(8),
      flip({ padding: 12, boundary: workspaceBoundary }),
      shift({ padding: 12, boundary: workspaceBoundary, crossAxis: true }),
      size({ padding: 12, boundary: workspaceBoundary, apply({ availableWidth, elements }) { elements.floating.style.maxWidth = `${Math.min(540, availableWidth)}px`; } }),
    ],
  });
  const click = useClick(context);
  const dismiss = useDismiss(context);
  const role = useRole(context, { role: 'dialog' });
  const { getReferenceProps, getFloatingProps } = useInteractions([click, dismiss, role]);
  const kinds: [NoteKind, string, typeof StickyNote][] = [['note', '备注', StickyNote], ['example', '例子', Quote], ['formula', '公式', Sigma], ['image', '图片', ImageIcon]];

  const readImage = (file: File) => {
    if (savingRef.current) return;
    if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) { setError('请选择 PNG、JPEG 或 WebP 图片。'); return; }
    if (file.size > 5 * 1024 * 1024) { setError('请选择小于 5 MB 的图片。'); return; }
    const reader = new FileReader();
    reader.onload = () => { setImage(String(reader.result)); setError(''); };
    reader.onerror = () => setError('无法读取这张图片。');
    reader.readAsDataURL(file);
  };
  const submit = async () => {
    if (savingRef.current || (kind === 'image' ? !image : !text.trim())) return;
    savingRef.current = true;
    setSaving(true); setError('');
    const saved = await onSubmit({ kind, text, sourceLabel: source.trim(), imageData: kind === 'image' ? image : null });
    savingRef.current = false;
    setSaving(false);
    if (!saved) { setError('资料保存失败，请保留当前内容并重试。'); return; }
    setText(''); setImage(null); setError(''); onOpenChange(false);
  };

  return <span className="inline-insert">
    <button ref={refs.setReference} className="insert-button" {...getReferenceProps()} aria-expanded={open}><Plus size={15} /> 添加资料</button>
    <AnimatePresence>
      {open && <FloatingPortal>
        <FloatingFocusManager context={context} modal={false} initialFocus={fieldRef} returnFocus>
          <motion.form
            ref={refs.setFloating}
            style={floatingStyles}
            className="note-composer glass-surface"
            {...getFloatingProps()}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ type: 'spring', bounce: 0, duration: reduceMotion ? .12 : .32 }}
            onSubmit={(event) => { event.preventDefault(); if (!composing.current && !(event.nativeEvent as Event & { isComposing?: boolean }).isComposing) void submit(); }}
            onPaste={(event) => { if (savingRef.current) { event.preventDefault(); return; } const file = Array.from(event.clipboardData.files).find((item) => item.type.startsWith('image/')); if (file) { event.preventDefault(); setKind('image'); readImage(file); } }}
            onKeyDown={(event) => { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && !composing.current && !event.nativeEvent.isComposing) { event.preventDefault(); void submit(); } }}
          >
            <div className="composer-heading"><strong>添加课堂资料</strong><button type="button" className="icon-button" aria-label="关闭资料编辑" disabled={saving} onClick={() => guardedOpenChange(false)}><X size={17} /></button></div>
            <div className="kind-picker" aria-label="资料类型">{kinds.map(([value, label, Icon]) => <button type="button" key={value} disabled={saving} className={kind === value ? 'selected' : ''} aria-pressed={kind === value} onClick={() => { setKind(value); if (value !== 'image') requestAnimationFrame(() => fieldRef.current?.focus()); }}><Icon size={16} />{label}</button>)}</div>
            {kind === 'image' && <label className={`image-drop ${image ? 'has-image' : ''}`} onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); if (savingRef.current) return; const file = event.dataTransfer.files[0]; if (file) readImage(file); }}>
              {image ? <img src={image} alt="待添加的课堂图片预览" /> : <><ImageIcon size={22} /><span>拖入、粘贴或选择图片</span><small>PNG、JPEG、WebP · 最大 5 MB</small></>}
              <input type="file" disabled={saving} accept="image/png,image/jpeg,image/webp" onChange={(event) => { const file = event.target.files?.[0]; if (file) readImage(file); }} />
            </label>}
            {kind === 'image' && image && <FormulaRecognitionPanel imageData={image} configured={configured} disabled={saving} onUse={(latex) => { setKind('formula'); setText(latex); setSource('Image formula · DeepSeek'); requestAnimationFrame(() => fieldRef.current?.focus()); }} />}
            <label className="composer-field"><span>{kind === 'formula' ? 'LaTeX 源码' : kind === 'image' ? '图片说明与替代文本' : '内容'}</span><textarea ref={fieldRef} disabled={saving} value={text} onChange={(event) => setText(event.target.value)} onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }} placeholder={kind === 'formula' ? '\\int_0^1 x^2 \, dx' : kind === 'example' ? '写下完整例子…' : kind === 'image' ? '说明图片中的内容…' : '写下备注…'} /></label>
            {kind === 'formula' && text && <Formula latex={text} />}
            <label className="composer-field composer-source"><span>来源</span><input disabled={saving} value={source} onChange={(event) => setSource(event.target.value)} onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }} placeholder="我的备注" /></label>
            {error && <p className="inline-error"><CircleAlert size={14} />{error}</p>}
            <footer><span>{saving ? '正在保存…' : `${shortcutLabel('Enter')} 添加`}</span><button type="button" className="secondary-button" disabled={saving} onClick={() => guardedOpenChange(false)}>取消</button><button className="primary-button" disabled={saving || (kind === 'image' ? !image : !text.trim())}>{saving ? '正在保存…' : '添加资料'}</button></footer>
          </motion.form>
        </FloatingFocusManager>
      </FloatingPortal>}
    </AnimatePresence>
  </span>;
}

function ReviewDialog({ segment, error, onResolve, onClose }: { segment: Segment; error: string; onResolve: (action: 'keep' | 'machine', text?: string) => void; onClose: () => void }) {
  const [manual, setManual] = useState(segment.displayText);
  return <Dialog title="核对识别更新" description={`${fmtTime(segment.startSample)} · 作出选择前，当前文字会继续保留。`} onClose={onClose} wide>
    <div className="compare-grid"><section><span>当前文字</span><p>{segment.displayText}</p></section><section><span>新识别版本</span><p>{segment.pendingMachine}</p></section></div>
    <label className="form-field"><span>手动合并</span><textarea value={manual} onChange={(e) => setManual(e.target.value)} /></label>
    {error && <p className="inline-error" role="alert"><CircleAlert size={14} />{error}</p>}
    <footer className="review-actions"><button className="secondary-button" onClick={() => onResolve('keep')}>保留当前文字</button><button className="secondary-button" onClick={() => onResolve('machine')}>采用识别版本</button><button className="primary-button" onClick={() => onResolve('keep', manual)}>保存手动合并</button></footer>
  </Dialog>;
}

/** Cheap change marker: every domain and runtime session mutation bumps operationSeq; the rest covers app-level fields. */
const stateSignature = (value: AppState) => `${value.selectedSessionId}|${value.workerEpoch}|${JSON.stringify(value.settings)}|${JSON.stringify(value.projects)}|${value.sessions.map((session) => `${session.id}:${session.operationSeq}:${session.title}:${session.projectId}:${session.recordingState}:${session.inferenceState}:${session.error}:${session.segments.length}:${session.notes.length}:${session.drafts.length}:${session.runs.map((run) => `${run.samples}/${run.state}`).join(',')}:${JSON.stringify(session.customVocabulary ?? [])}`).join('|')}`;

export default function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [runtime, setRuntime] = useState<RuntimeInfo>({ modelState: 'loading' });
  const [fatal, setFatal] = useState(''); const [notice, setNotice] = useState(''); const noticeTimer = useRef<number | undefined>(undefined);
  const fatalOrigin = useRef<{ source: 'action' | 'poll'; request: number } | null>(null); const dismissedPollError = useRef('');
  const [dialogError, setDialogError] = useState(''); const [commitError, setCommitError] = useState('');
  const creatingCourseRef = useRef(false); const [creatingCourse, setCreatingCourse] = useState(false);
  const stateRef = useRef(state); stateRef.current = state;
  const [modal, setModal] = useState<Modal>(null); const [newTitle, setNewTitle] = useState(''); const [newProjectId, setNewProjectId] = useState<string | null>(null);
  const modalRef = useRef(modal); modalRef.current = modal;
  useEffect(() => setDialogError(''), [modal]);
  const [source, setSource] = useState<'microphone' | 'system'>('microphone');
  const [captureActionPending, setCaptureActionPending] = useState<'start' | 'pause' | 'resume' | 'stop' | null>(null);
  const [cloudActionPending, setCloudActionPending] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false); const [pendingSearchTarget, setPendingSearchTarget] = useState<{ result: SearchResult; play: boolean; generation: number } | null>(null); const [fontSize, setFontSize] = useState(19);
  const [sidebarExpanded, setSidebarExpanded] = useState(() => { try { const saved = localStorage.getItem('lectureedit.sidebar-expanded'); return saved === null ? window.innerWidth > 1100 : saved === '1'; } catch { return true; } });
  const [sidebarRevealProjectId, setSidebarRevealProjectId] = useState<string | null>(null);
  const [engineOpen, setEngineOpen] = useState(false); const [pendingEngine, setPendingEngine] = useState<'qwen' | 'soniox' | undefined>(undefined);
  const [viewMode, setViewMode] = useState<'following' | 'reading_history'>('following'); const [unseen, setUnseen] = useState(0);
  const viewModeRef = useRef(viewMode); viewModeRef.current = viewMode;
  const [editor, setEditor] = useState<{ draft: Draft; text: string; revision: number; originRect?: SegmentRect; exiting?: boolean } | null>(null); const editorRef = useRef(editor); editorRef.current = editor;
  const [fullEditor, setFullEditor] = useState<{ sessionId: string; segments: Segment[]; original: TranscriptTextMap; values: TranscriptTextMap; status: 'editing' | 'saving' | 'saved' | 'error'; error: string } | null>(null); const fullEditorRef = useRef(fullEditor); fullEditorRef.current = fullEditor;
  const [saveStatus, setSaveStatus] = useState<SaveStatus>('idle'); const saveStatusRef = useRef(saveStatus); saveStatusRef.current = saveStatus; const saveTimer = useRef<number | undefined>(undefined); const saveChain = useRef<Promise<void>>(Promise.resolve());
  const [noteSegment, setNoteSegment] = useState<string | null>(null); const [noteKind, setNoteKind] = useState<NoteKind>('note'); const [reviewSegment, setReviewSegment] = useState<string | null>(null);
  const transcriptRef = useRef<HTMLDivElement>(null); const transcriptContentRef = useRef<HTMLDivElement>(null); const followAnchorRef = useRef<HTMLDivElement>(null);
  const lastScrollTop = useRef(0); const lastScrollHeight = useRef(0); const lastClientHeight = useRef(0); const programmaticScrollRef = useRef(false); const scrollGuardTimer = useRef<number | undefined>(undefined); const scrollAnimationRef = useRef<ReturnType<typeof animate> | null>(null);
  const followGoalRef = useRef<{ goal: number; viewport: number } | null>(null); const followFrameRef = useRef<number | undefined>(undefined); const pauseFollowingRef = useRef<() => void>(() => undefined);
  const pointerDownRef = useRef(false); const touchStartYRef = useRef<number | null>(null);
  const transcriptStateRef = useRef<{ sessionId: string | null; count: number; revision: string }>({ sessionId: null, count: 0, revision: '' }); const seenSegmentsRef = useRef<Set<string>>(new Set()); const seenSessionRef = useRef<string | null>(null); const groupingRef = useRef<{ sessionId: string | null; state: TranscriptGroupingState }>({ sessionId: null, state: { known: new Set(), paragraphStarts: new Set() } }); const [exportOpen, setExportOpen] = useState(false); const [moreOpen, setMoreOpen] = useState(false);
  const engineButtonRef = useRef<HTMLButtonElement>(null); const exportButtonRef = useRef<HTMLButtonElement>(null); const moreButtonRef = useRef<HTMLButtonElement>(null);
  const engineMenuRef = useRef<HTMLDivElement>(null); const exportMenuRef = useRef<HTMLDivElement>(null); const engineWrapRef = useRef<HTMLDivElement>(null); const exportWrapRef = useRef<HTMLDivElement>(null); const moreWrapRef = useRef<HTMLDivElement>(null);
  const editHistory = useRef<{ items: string[]; index: number }>({ items: [], index: 0 }); const defaultsApplied = useRef(false);
  const requestSequence = useRef(0); const appliedSequence = useRef(0); const pendingWrites = useRef(0);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const playbackRequest = useRef(0);
  const searchNavigationGeneration = useRef(0);
  const [playingSegmentId, setPlayingSegmentId] = useState<string | null>(null);
  const selected = state?.sessions.find((s) => s.id === state.selectedSessionId) ?? null;
  const selectedProject = selected?.projectId ? state?.projects.find((project) => project.id === selected.projectId) ?? null : null;
  const transcriptGroups = useMemo(() => {
    const sessionId = selected?.id ?? null;
    if (groupingRef.current.sessionId !== sessionId) groupingRef.current = { sessionId, state: { known: new Set(), paragraphStarts: new Set() } };
    return groupTranscriptSegments(selected?.segments ?? [], groupingRef.current.state);
  }, [selected?.id, selected?.segments]);
  const selectedIdRef = useRef<string | null>(selected?.id ?? null); selectedIdRef.current = selected?.id ?? null;
  const activeSegment = selected?.segments.find((s) => s.id === editor?.draft.segmentId) ?? null;
  const totalSamples = useMemo(() => selected?.runs.reduce((sum, run) => sum + run.samples, 0) ?? 0, [selected?.runs]);
  const wordCount = useMemo(() => countWords(selected?.segments ?? []), [selected?.segments]);
  const [introHidden, setIntroHidden] = useState(false);
  const hasTranscript = Boolean(selected?.segments.length);
  useEffect(() => {
    const root = transcriptRef.current;
    const intro = transcriptContentRef.current?.querySelector('.session-intro');
    if (!root || !intro || typeof IntersectionObserver === 'undefined') { setIntroHidden(false); return; }
    const observer = new IntersectionObserver(([entry]) => setIntroHidden(!entry.isIntersecting), { root, rootMargin: '-64px 0px 0px 0px' });
    observer.observe(intro);
    return () => observer.disconnect();
  }, [selected?.id, hasTranscript, fullEditor?.sessionId]);
  useEffect(() => {
    const preference = state?.settings.theme ?? 'system';
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      const resolved = preference === 'system' ? (media.matches ? 'dark' : 'light') : preference;
      document.documentElement.dataset.theme = resolved;
      document.documentElement.style.colorScheme = resolved;
    };
    apply();
    if (preference === 'system') media.addEventListener('change', apply);
    return () => media.removeEventListener('change', apply);
  }, [state?.settings.theme]);
  useEffect(() => { localStorage.setItem('lectureedit.sidebar-expanded', sidebarExpanded ? '1' : '0'); }, [sidebarExpanded]);
  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if (event.isComposing || modalRef.current || fullEditorRef.current || !isPrimaryShortcut(event)) return;
      if (event.key.toLocaleLowerCase() === 'k' || event.key.toLocaleLowerCase() === 'f') { event.preventDefault(); setSearchOpen(true); }
    };
    window.addEventListener('keydown', shortcut);
    return () => window.removeEventListener('keydown', shortcut);
  }, []);
  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing || (!engineOpen && !exportOpen && !moreOpen)) return;
      event.preventDefault();
      event.stopPropagation();
      const trigger = engineOpen ? engineButtonRef.current : exportOpen ? exportButtonRef.current : moreButtonRef.current;
      setEngineOpen(false); setExportOpen(false); setMoreOpen(false);
      requestAnimationFrame(() => trigger?.focus());
    };
    const closeOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if ([engineWrapRef.current, exportWrapRef.current, moreWrapRef.current].some((element) => element?.contains(target))) return;
      setEngineOpen(false); setExportOpen(false); setMoreOpen(false);
    };
    window.addEventListener('keydown', closeOnEscape, true);
    document.addEventListener('pointerdown', closeOutside);
    return () => { window.removeEventListener('keydown', closeOnEscape, true); document.removeEventListener('pointerdown', closeOutside); };
  }, [engineOpen, exportOpen, moreOpen]);
  const transcriptRevision = selected?.segments.map((segment) => `${segment.id}:${segment.machineRevision}:${segment.userSeq}:${segment.final}:${segment.displayText}`).join('|') ?? '';

  const showError = useCallback((error: unknown, source: 'action' | 'poll' = 'action') => {
    const message = error instanceof Error ? error.message : String(error);
    if (source === 'poll' && message === dismissedPollError.current) return;
    fatalOrigin.current = { source, request: requestSequence.current }; setFatal(message);
  }, []);
  const clearFatal = useCallback((source: 'action' | 'poll', request: number) => {
    const origin = fatalOrigin.current;
    if (origin?.source === source && request > origin.request) { fatalOrigin.current = null; setFatal(''); }
  }, []);
  const dismissFatal = () => { if (fatalOrigin.current?.source === 'poll') dismissedPollError.current = fatal; fatalOrigin.current = null; setFatal(''); };
  const flashNotice = useCallback((message: string, duration: number) => {
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    setNotice(message); noticeTimer.current = window.setTimeout(() => { noticeTimer.current = undefined; setNotice(''); }, duration);
  }, []);
  const stopPlayback = useCallback(() => { playbackRequest.current += 1; audioRef.current?.pause(); audioRef.current = null; setPlayingSegmentId(null); }, []);
  const guardProgrammaticScroll = useCallback((duration = 500) => {
    programmaticScrollRef.current = true;
    if (scrollGuardTimer.current) clearTimeout(scrollGuardTimer.current);
    scrollGuardTimer.current = window.setTimeout(() => {
      programmaticScrollRef.current = false;
      const element = transcriptRef.current;
      if (element) { lastScrollTop.current = element.scrollTop; lastScrollHeight.current = element.scrollHeight; lastClientHeight.current = element.clientHeight; }
    }, duration);
  }, []);
  const getFollowTarget = useCallback(() => {
    const element = transcriptRef.current;
    const anchor = followAnchorRef.current;
    if (!element || !anchor) return null;
    const viewport = element.getBoundingClientRect();
    const anchorTop = anchor.getBoundingClientRect().top - viewport.top + element.scrollTop;
    return transcriptFollowTarget(anchorTop, element.scrollHeight, element.clientHeight);
  }, []);
  /** `background` writes (long model downloads) don't pause snapshot polling; `onError` reports inline instead of the banner. */
  const update = useCallback(async (promise: Promise<AppState | null>, options: { onError?: (message: string) => void; background?: boolean } = {}) => {
    const request = ++requestSequence.current; if (!options.background) pendingWrites.current += 1;
    try {
      const result = await promise;
      if (result && request >= appliedSequence.current) { appliedSequence.current = request; setState(result); }
      if (result) clearFatal('action', request);
      return result;
    } catch (error) { if (options.onError) options.onError(error instanceof Error ? error.message : String(error)); else showError(error); return null; }
    finally { if (!options.background) pendingWrites.current = Math.max(0, pendingWrites.current - 1); }
  }, [clearFatal, showError]);

  useEffect(() => { void update(adapter.dispatch({ type: 'snapshot' })); adapter.runtimeInfo().then(setRuntime).catch(showError); }, [showError, update]);
  useEffect(() => {
    if (!state || defaultsApplied.current || !runtime.defaults) return;
    const merged = { ...state.settings };
    for (const key of ['engine', 'executable', 'modelPath', 'mmprojPath', 'language'] as const) if (!merged[key] && runtime.defaults[key]) merged[key] = runtime.defaults[key] as string;
    defaultsApplied.current = true;
    if (JSON.stringify(merged) !== JSON.stringify(state.settings)) void update(adapter.dispatch({ type: 'settings', commandId: commandId(), settings: merged }));
  }, [runtime.defaults, state, update]);
  useEffect(() => { const timer = window.setInterval(() => {
    adapter.runtimeInfo().then(setRuntime).catch(() => undefined);
    if (pendingWrites.current > 0) return;
    const request = ++requestSequence.current;
    adapter.dispatch({ type: adapter.mode === 'demo' ? 'demoTick' : 'snapshot', ...(selected ? { sessionId: selected.id } : {}) }).then((next) => {
      if (pendingWrites.current === 0 && request >= appliedSequence.current) {
        appliedSequence.current = request;
        if (!stateRef.current || stateSignature(next) !== stateSignature(stateRef.current)) setState(next);
      }
      clearFatal('poll', request);
    }).catch((error) => showError(error, 'poll'));
  }, 700); return () => clearInterval(timer); }, [selected?.id, clearFatal, showError]);

  const queueDraftSave = useCallback((payload: { draft: Draft; text: string; revision: number }) => {
    setSaveStatus('saving');
    pendingWrites.current += 1;
    const work = async () => {
      const request = ++requestSequence.current;
      try {
        const next = await adapter.dispatch({ type: 'saveDraft', commandId: commandId(), sessionId: state?.selectedSessionId, draftId: payload.draft.id, text: payload.text, revision: payload.revision });
        if (request >= appliedSequence.current) { appliedSequence.current = request; setState(next); }
        setSaveStatus('saved');
      } catch (error) { setSaveStatus('error'); showError(error); throw error; }
      finally { pendingWrites.current = Math.max(0, pendingWrites.current - 1); }
    };
    saveChain.current = saveChain.current.catch(() => undefined).then(work);
    return saveChain.current;
  }, [showError, state?.selectedSessionId]);

  const flushDraft = useCallback(async () => {
    let saved = true;
    const current = editorRef.current;
    if (saveTimer.current) {
      clearTimeout(saveTimer.current); saveTimer.current = undefined;
      if (current) try { await queueDraftSave(current); } catch { saved = false; }
    } else if (saveStatusRef.current === 'error' && current) {
      try { await queueDraftSave(current); } catch { saved = false; }
    }
    try { await saveChain.current; } catch { saved = false; }
    return saved;
  }, [queueDraftSave]);
  useEffect(() => () => {
    if (saveTimer.current) clearTimeout(saveTimer.current);
    if (noticeTimer.current) clearTimeout(noticeTimer.current);
    if (scrollGuardTimer.current) clearTimeout(scrollGuardTimer.current);
  }, []);
  useEffect(() => () => stopPlayback(), [stopPlayback]);
  useEffect(() => { const onBlur = () => { void flushDraft(); }; window.addEventListener('blur', onBlur); return () => window.removeEventListener('blur', onBlur); }, [flushDraft]);

  const setDraftText = (text: string, recordHistory: boolean) => {
    if (!editor) return; const next = { ...editor, text, revision: editor.revision + 1 }; setEditor(next); setSaveStatus('dirty');
    if (recordHistory) { const history = editHistory.current; history.items = [...history.items.slice(0, history.index + 1), text]; history.index = history.items.length - 1; }
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => { saveTimer.current = undefined; void queueDraftSave(next); }, 300);
  };
  const changeDraft = (text: string) => setDraftText(text, true);
  const undoDraft = () => { const history = editHistory.current; if (history.index <= 0) return; history.index -= 1; setDraftText(history.items[history.index], false); };
  const redoDraft = () => { const history = editHistory.current; if (history.index >= history.items.length - 1) return; history.index += 1; setDraftText(history.items[history.index], false); };
  const beginEdit = async (segment: Segment) => {
    stopPlayback();
    setViewMode('reading_history'); setNoteSegment(null);
    if (!await flushDraft()) return;
    const sourceNode = Array.from(document.querySelectorAll<HTMLElement>('[data-segment]')).find((node) => node.dataset.segment === segment.id)?.querySelector<HTMLElement>('.segment-copy');
    const sourceRect = sourceNode?.getBoundingClientRect();
    const originRect = sourceRect ? { left: sourceRect.left, top: sourceRect.top, width: sourceRect.width, height: sourceRect.height } : undefined;
    const recoveryCandidates = selected?.drafts.filter((draft) => draft.segmentId === segment.id && !['committed', 'discarded'].includes(draft.state) && draft.text !== draft.snapshotText) ?? [];
    const recovered = recoveryCandidates[recoveryCandidates.length - 1];
    const next = await update(adapter.dispatch({ type: 'beginEdit', commandId: commandId(), sessionId: selected?.id, segmentId: segment.id }));
    const drafts = next?.sessions.find((s) => s.id === selected?.id)?.drafts.filter((draft) => draft.segmentId === segment.id && !['committed', 'discarded'].includes(draft.state)) ?? [];
    const draft = drafts[drafts.length - 1];
    if (draft) {
      const shouldRestore = draft.text !== draft.snapshotText || Boolean(recovered);
      const text = draft.text !== draft.snapshotText ? draft.text : recovered?.text ?? draft.text;
      const revision = recovered && recovered.id !== draft.id && text !== draft.text ? draft.revision + 1 : draft.revision;
      const local = { draft, text, revision, originRect };
      setEditor(local); editHistory.current = { items: [text], index: 0 }; setSaveStatus(shouldRestore ? 'restored' : 'idle');
      if (revision > draft.revision) saveTimer.current = window.setTimeout(() => { saveTimer.current = undefined; void queueDraftSave(local); }, 600);
    }
  };
  const animateEditorHome = async () => {
    const current = editorRef.current;
    if (current?.originRect && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      setEditor({ ...current, exiting: true });
      await new Promise<void>((resolve) => window.setTimeout(resolve, 400));
    }
    setEditor(null);
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  };
  const closeEditor = async () => { if (!await flushDraft()) return; if (editor && selected) { const result = await update(adapter.dispatch({ type: 'closeDraft', commandId: commandId(), sessionId: selected.id, draftId: editor.draft.id })); if (!result) return; } await animateEditorHome(); };
  const discardEditor = async () => { if (!editor || !selected) return; if (saveTimer.current) { clearTimeout(saveTimer.current); saveTimer.current = undefined; } await saveChain.current.catch(() => undefined); const result = await update(adapter.dispatch({ type: 'discardDraft', commandId: commandId(), sessionId: selected.id, draftId: editor.draft.id })); if (result) await animateEditorHome(); };
  const commitEditor = async () => { if (!editor || !selected) return; if (!await flushDraft()) return; setSaveStatus('saving'); setCommitError(''); const wasRecording = selected.recordingState === 'recording'; const result = await update(adapter.dispatch({ type: 'commitEdit', commandId: commandId(), sessionId: selected.id, draftId: editor.draft.id, text: editor.text, expectedUserSeq: editor.draft.expectedUserSeq }), { onError: setCommitError }); if (result) { setSaveStatus('saved'); if (wasRecording) guardProgrammaticScroll(1200); await animateEditorHome(); if (wasRecording) returnLatest(); flashNotice(activeSegment?.pendingMachine ? '修订已保存，另有识别更新待核对。' : '修订已保存。', 2400); } else setSaveStatus('commitError'); };

  const beginFullEdit = async () => {
    if (!selected || ['starting', 'recording', 'paused', 'stopping'].includes(selected.recordingState) || !await flushDraft()) return;
    stopPlayback(); setNoteSegment(null); setViewMode('reading_history');
    const values = transcriptTextMap(selected.segments);
    setFullEditor({ sessionId: selected.id, segments: selected.segments.map((segment) => ({ ...segment })), original: values, values: { ...values }, status: 'editing', error: '' });
  };
  const saveFullDocument = async (exitWhenSaved = false) => {
    const current = fullEditorRef.current;
    if (!current) return true;
    const changed = changedTranscriptSegments(current.segments, current.original, current.values);
    if (!changed.length) { if (exitWhenSaved) setFullEditor(null); return true; }
    setFullEditor((value) => value ? { ...value, status: 'saving', error: '' } : value);
    pendingWrites.current += 1;
    const request = ++requestSequence.current;
    try {
      const result = await persistTranscriptEdits(adapter, current.sessionId, current.segments, current.original, current.values);
      if (result.state && request >= appliedSequence.current) { appliedSequence.current = request; setState(result.state); }
      const nextOriginal = { ...current.original };
      for (const id of result.savedSegmentIds) nextOriginal[id] = current.values[id];
      if (exitWhenSaved) setFullEditor(null);
      else setFullEditor((value) => value && value.sessionId === current.sessionId ? { ...value, original: nextOriginal, status: 'saved', error: '' } : value);
      flashNotice('全文修改已保存到本机。', 2400);
      return true;
    } catch (cause) {
      const failure = cause instanceof TranscriptSaveError ? cause : new TranscriptSaveError(cause instanceof Error ? cause.message : String(cause), null, []);
      if (failure.latestState && request >= appliedSequence.current) { appliedSequence.current = request; setState(failure.latestState); }
      setFullEditor((value) => {
        if (!value || value.sessionId !== current.sessionId) return value;
        const nextOriginal = { ...value.original };
        for (const id of failure.savedSegmentIds) nextOriginal[id] = value.values[id];
        return { ...value, original: nextOriginal, status: 'error', error: failure.message };
      });
      return false;
    } finally { pendingWrites.current = Math.max(0, pendingWrites.current - 1); }
  };
  useEffect(() => {
    const saveShortcut = (event: KeyboardEvent) => {
      if (!fullEditorRef.current || modalRef.current || event.isComposing || !isPrimaryShortcut(event) || event.key.toLocaleLowerCase() !== 's') return;
      event.preventDefault(); void saveFullDocument();
    };
    window.addEventListener('keydown', saveShortcut);
    return () => window.removeEventListener('keydown', saveShortcut);
  });

  useEffect(() => {
    const sessionId = selected?.id ?? null;
    const count = selected?.segments.length ?? 0;
    const previous = transcriptStateRef.current;
    if (previous.sessionId !== sessionId) {
      setUnseen(0);
    } else if (previous.revision && previous.revision !== transcriptRevision && viewMode === 'reading_history') {
      const added = Math.max(0, count - previous.count);
      setUnseen((current) => added > 0 ? current + added : Math.max(1, current));
    }
    transcriptStateRef.current = { sessionId, count, revision: transcriptRevision };
    if (seenSessionRef.current !== sessionId) { seenSessionRef.current = sessionId; seenSegmentsRef.current = new Set(selected?.segments.map((segment) => segment.id) ?? []); }
    else selected?.segments.forEach((segment) => seenSegmentsRef.current.add(segment.id));
  }, [selected?.id, selected?.segments, transcriptRevision, viewMode]);
  const stopFollowGlide = useCallback(() => {
    if (followFrameRef.current !== undefined) cancelAnimationFrame(followFrameRef.current);
    followFrameRef.current = undefined;
  }, []);
  const syncFollowPosition = useCallback(() => {
    if (viewModeRef.current !== 'following' || editorRef.current) return;
    const element = transcriptRef.current;
    const target = getFollowTarget();
    if (!element || target === null) return;
    const viewport = element.clientHeight;
    const previous = followGoalRef.current;
    // A resized window re-anchors; otherwise the goal only advances (see nextFollowGoal).
    const goal = nextFollowGoal(previous && previous.viewport === viewport ? previous.goal : null, target, viewport);
    followGoalRef.current = { goal, viewport };
    const settle = () => {
      lastScrollTop.current = element.scrollTop;
      lastScrollHeight.current = element.scrollHeight;
      lastClientHeight.current = element.clientHeight;
    };
    if (Math.abs(goal - element.scrollTop) < .5) { settle(); return; }
    scrollAnimationRef.current?.stop();
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    // First placement (opening a lesson, resizing) is instant; live growth glides.
    if (!previous || previous.viewport !== viewport || reduce) {
      stopFollowGlide();
      guardProgrammaticScroll(80);
      element.scrollTop = goal;
      settle();
      return;
    }
    if (followFrameRef.current !== undefined) return;
    programmaticScrollRef.current = true;
    let last = performance.now();
    let placed = element.scrollTop;
    const tick = (now: number) => {
      const current = followGoalRef.current;
      if (viewModeRef.current !== 'following' || !current) { followFrameRef.current = undefined; return; }
      // Dragging the scrollbar mid-glide: hand the page back to the reader.
      if (pointerDownRef.current && Math.abs(element.scrollTop - placed) > 2) { followFrameRef.current = undefined; programmaticScrollRef.current = false; pauseFollowingRef.current(); return; }
      element.scrollTop = followStep(element.scrollTop, current.goal, now - last);
      placed = element.scrollTop;
      last = now;
      settle();
      if (Math.abs(current.goal - element.scrollTop) < .5) {
        followFrameRef.current = undefined;
        guardProgrammaticScroll(80);
        return;
      }
      followFrameRef.current = requestAnimationFrame(tick);
    };
    followFrameRef.current = requestAnimationFrame(tick);
  }, [getFollowTarget, guardProgrammaticScroll, stopFollowGlide]);
  useLayoutEffect(() => {
    const content = transcriptContentRef.current;
    const element = transcriptRef.current;
    if (!content || !element) return;
    const observer = new ResizeObserver(syncFollowPosition);
    observer.observe(content);
    observer.observe(element);
    syncFollowPosition();
    return () => { observer.disconnect(); scrollAnimationRef.current?.stop(); stopFollowGlide(); followGoalRef.current = null; };
  }, [selected?.id, stopFollowGlide, syncFollowPosition]);
  useLayoutEffect(() => {
    syncFollowPosition();
  }, [syncFollowPosition, transcriptRevision, transcriptGroups]);
  const pauseFollowing = () => {
    if (viewModeRef.current !== 'following') return;
    viewModeRef.current = 'reading_history';
    programmaticScrollRef.current = false;
    if (scrollGuardTimer.current) { clearTimeout(scrollGuardTimer.current); scrollGuardTimer.current = undefined; }
    scrollAnimationRef.current?.stop(); stopFollowGlide(); followGoalRef.current = null;
    setViewMode('reading_history'); setNoteSegment(null);
  };
  pauseFollowingRef.current = pauseFollowing;
  const returnLatest = () => {
    viewModeRef.current = 'following';
    setViewMode('following'); setUnseen(0);
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const element = transcriptRef.current;
    const target = getFollowTarget();
    if (!element || target === null) return;
    scrollAnimationRef.current?.stop(); stopFollowGlide();
    followGoalRef.current = { goal: target, viewport: element.clientHeight };
    guardProgrammaticScroll(650);
    if (reduce) element.scrollTop = target;
    else scrollAnimationRef.current = animate(element.scrollTop, target, { type: 'spring', bounce: 0, duration: .34, onUpdate: (value) => { element.scrollTop = value; } });
  };
  const handleTranscriptScroll = (event: React.UIEvent<HTMLDivElement>) => {
    const element = event.currentTarget;
    const shouldPause = shouldPauseTranscriptFollow({
      currentScrollTop: element.scrollTop,
      previousScrollTop: lastScrollTop.current,
      currentScrollHeight: element.scrollHeight,
      previousScrollHeight: lastScrollHeight.current,
      currentViewportHeight: element.clientHeight,
      previousViewportHeight: lastClientHeight.current,
      programmatic: programmaticScrollRef.current,
      pointerDown: pointerDownRef.current,
    });
    lastScrollTop.current = element.scrollTop;
    lastScrollHeight.current = element.scrollHeight;
    lastClientHeight.current = element.clientHeight;
    if (shouldPause) pauseFollowing();
  };
  const handleTranscriptKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (['ArrowUp', 'PageUp', 'Home'].includes(event.key)) pauseFollowing();
  };
  const handleTranscriptTouchStart = (event: React.TouchEvent<HTMLDivElement>) => {
    touchStartYRef.current = event.touches[0]?.clientY ?? null;
  };
  const handleTranscriptTouchMove = (event: React.TouchEvent<HTMLDivElement>) => {
    const start = touchStartYRef.current;
    const current = event.touches[0]?.clientY;
    if (start !== null && current !== undefined && current - start > 8) { pauseFollowing(); touchStartYRef.current = null; }
  };
  const editCurrentSentence = () => {
    if (editorRef.current) return;
    const current = selected?.segments.at(-1);
    if (!current) return;
    scrollAnimationRef.current?.stop();
    setViewMode('reading_history');
    setNoteSegment(null);
    void beginEdit(current);
  };
  useEffect(() => {
    const editShortcut = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const typing = target?.matches('input, textarea, select, [contenteditable="true"]');
      if (event.isComposing || typing || editorRef.current || modalRef.current || fullEditorRef.current || !isPrimaryShortcut(event) || event.key.toLocaleLowerCase() !== 'e') return;
      if (!selected?.segments.length) return;
      event.preventDefault();
      editCurrentSentence();
    };
    window.addEventListener('keydown', editShortcut);
    return () => window.removeEventListener('keydown', editShortcut);
  }, [selected?.id, transcriptRevision]);

  const playSegment = async (segment: Segment, anchor?: SearchResult['audioAnchor'], playbackKey = segment.id) => {
    if (!selected) return;
    if (playingSegmentId === playbackKey && audioRef.current) { stopPlayback(); return; }
    stopPlayback();
    const request = playbackRequest.current;
    const startSample = Math.max(0, (anchor?.startSample ?? segment.startSample) - 8000);
    const endSample = Math.min((anchor?.endSample ?? segment.endSample) + 8000, startSample + 16000 * 60);
    try {
      const encoded = await adapter.audioData(selected.id, anchor?.runId ?? segment.runId, startSample, endSample);
      if (request !== playbackRequest.current) return;
      const audio = new Audio(encoded.startsWith('data:') ? encoded : `data:audio/wav;base64,${encoded}`);
      audioRef.current = audio;
      const clear = () => { if (audioRef.current === audio) { audioRef.current = null; setPlayingSegmentId(null); } };
      audio.onended = clear; audio.onerror = clear; audio.onpause = clear;
      setPlayingSegmentId(playbackKey);
      await audio.play();
    } catch (error) { if (request === playbackRequest.current) { stopPlayback(); showError(error); } }
  };
  const moveSegmentHistory = async (segment: Segment, direction: 'undo' | 'redo') => {
    stopPlayback();
    const result = await update(adapter.dispatch({ type: direction, commandId: commandId(), sessionId: selected?.id, segmentId: segment.id, expectedUserSeq: segment.userSeq }));
    if (!result) return;
    flashNotice(direction === 'undo' ? '已撤销此句最近一次修订。' : '已重做此句修订。', 1800);
  };
  const createCourse = async () => {
    if (creatingCourseRef.current) return;
    creatingCourseRef.current = true; setCreatingCourse(true); setDialogError('');
    try { if (!await saveFullDocument(true) || !await flushDraft()) return; searchNavigationGeneration.current += 1; setPendingSearchTarget(null); stopPlayback(); const title = newTitle.trim() || `${new Intl.DateTimeFormat('zh-CN', { month: 'long', day: 'numeric' }).format(Date.now())}课堂`; const result = await update(adapter.dispatch({ type: 'createSession', commandId: commandId(), title, projectId: newProjectId, mode: adapter.mode === 'demo' ? 'demo' : 'live' }), { onError: setDialogError }); if (result) { setEditor(null); setModal(null); setNewTitle(''); setNewProjectId(null); viewModeRef.current = 'following'; setViewMode('following'); setUnseen(0); } }
    finally { creatingCourseRef.current = false; setCreatingCourse(false); }
  };
  const prepareModel = async () => { const result = await update(adapter.dispatch({ type: 'prepareModel', commandId: commandId() })); try { setRuntime(await adapter.runtimeInfo()); } catch (error) { showError(error); } return Boolean(result); };
  const toggleRecording = async () => {
    if (!selected || captureActionPending) return;
    const isRecording = ['recording', 'starting', 'paused', 'stopping'].includes(selected.recordingState);
    if (!isRecording && !await saveFullDocument(true)) return;
    setCaptureActionPending(isRecording ? 'stop' : 'start');
    try {
      if (isRecording) await update(adapter.stopRecording(selected.id));
      else {
        if (adapter.mode === 'native' && startBlocked) return;
        stopPlayback();
        setViewMode('following'); setUnseen(0);
        await update(adapter.startRecording(selected.id, source));
      }
    } finally { setCaptureActionPending(null); }
  };
  const toggleRecordingPause = async () => {
    if (!selected || captureActionPending || !['recording', 'paused'].includes(selected.recordingState)) return;
    const resume = selected.recordingState === 'paused';
    setCaptureActionPending(resume ? 'resume' : 'pause');
    try {
      stopPlayback();
      if (resume) await update(adapter.startRecording(selected.id, source));
      else await update(adapter.pauseRecording(selected.id));
    } finally { setCaptureActionPending(null); }
  };
  const toggleCloudProcessing = async () => {
    if (!selected || cloudActionPending) return;
    setCloudActionPending(true);
    try {
      if (cloudActiveForSelected) await update(adapter.dispatch({ type: 'cancelCloud', commandId: commandId(), sessionId: selected.id }));
      else if (!cloudBusyElsewhere) await update(adapter.retryInference(selected.id));
      try { setRuntime(await adapter.runtimeInfo()); } catch { /* polling will refresh the status */ }
    } finally { setCloudActionPending(false); }
  };
  const setSelected = async (id: string) => { if (!await saveFullDocument(true) || !await flushDraft()) return; searchNavigationGeneration.current += 1; setPendingSearchTarget(null); stopPlayback(); const result = await update(adapter.dispatch({ type: 'selectSession', commandId: commandId(), sessionId: id })); if (result) { setEditor(null); setFullEditor(null); viewModeRef.current = 'following'; setViewMode('following'); setUnseen(0); } };
  const importCoursePackage = async () => { if (!await saveFullDocument(true) || !await flushDraft()) return; searchNavigationGeneration.current += 1; setPendingSearchTarget(null); stopPlayback(); const result = await update(adapter.importPackage()); if (result) { setEditor(null); setFullEditor(null); } };
  const createProject = async (title: string) => {
    const existing = new Set(state?.projects.map(project => project.id) ?? []);
    const result = await update(adapter.dispatch({ type: 'createProject', commandId: commandId(), title }));
    return result?.projects.find(project => !existing.has(project.id)) ?? null;
  };
  const renameProject = (projectId: string, title: string) => { const next = title.trim(); if (next) void update(adapter.dispatch({ type: 'renameProject', commandId: commandId(), projectId, title: next })); };
  const moveSession = (sessionId: string, projectId: string | null) => { void update(adapter.dispatch({ type: 'moveSession', commandId: commandId(), sessionId, projectId })); };
  const navigateSearch = async (result: SearchResult, play: boolean) => {
    if (!state?.sessions.some(session => session.id === result.sessionId) || !await saveFullDocument(true) || !await flushDraft()) return false;
    stopPlayback();
    if (state.selectedSessionId !== result.sessionId) {
      const next = await update(adapter.dispatch({ type: 'selectSession', commandId: commandId(), sessionId: result.sessionId }));
      if (!next) return false;
    }
    const generation = ++searchNavigationGeneration.current;
    const targetSession = state.sessions.find(session => session.id === result.sessionId);
    setSidebarExpanded(true); setSidebarRevealProjectId(targetSession?.projectId ?? null);
    setEditor(null); setFullEditor(null); setViewMode('reading_history'); setNoteSegment(null); setPendingSearchTarget({ result, play, generation });
    return true;
  };
  useEffect(() => {
    if (!pendingSearchTarget || selected?.id !== pendingSearchTarget.result.sessionId) return;
    const result = pendingSearchTarget.result;
    const generation = pendingSearchTarget.generation;
    const segment = result.segmentId ? selected.segments.find(item => item.id === result.segmentId) : null;
    requestAnimationFrame(() => {
      if (generation !== searchNavigationGeneration.current || selectedIdRef.current !== result.sessionId) return;
      const noteTarget = result.noteId ? transcriptContentRef.current?.querySelector<HTMLElement>(`[data-note="${CSS.escape(result.noteId)}"]`) : null;
      const target = noteTarget ?? (segment ? transcriptContentRef.current?.querySelector<HTMLElement>(`[data-segment="${CSS.escape(segment.id)}"]`) : transcriptContentRef.current?.querySelector<HTMLElement>('.session-intro'));
      target?.scrollIntoView({ block: 'center', behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
      target?.classList.add('search-target');
      if (target) window.setTimeout(() => target.classList.remove('search-target'), 1800);
      if (pendingSearchTarget.play && segment && result.audioAnchor) void playSegment(segment, result.audioAnchor);
    });
    setPendingSearchTarget(null);
  }, [pendingSearchTarget, selected?.id, selected?.segments]);
  const handlePickPath = async (key: 'executable' | 'modelPath' | 'mmprojPath') => { const filters = key === 'executable' ? undefined : [{ name: key === 'modelPath' ? '模型文件' : '音频投影文件', extensions: ['gguf'] }]; return adapter.pickFile(filters); };
  const pendingCount = selected?.segments.filter((s) => s.pendingMachine).length ?? 0;
  const reviewed = reviewSegment ? selected?.segments.find((s) => s.id === reviewSegment) : null;
  const captureInProgress = Boolean(selected && ['starting', 'recording', 'paused', 'stopping'].includes(selected.recordingState));
  const cloudEngine = state?.settings.engine === 'soniox';
  const legacyEngine = state?.settings.engine === 'whisper';
  const engineSwitchBlocked = captureInProgress || ['downloading', 'verifying'].includes(runtime.modelDownload?.phase ?? 'idle');
  const cloudActiveForSelected = Boolean(runtime.cloudProcessing && selected && runtime.cloudSessionId === selected.id);
  const cloudBusyElsewhere = Boolean(runtime.cloudProcessing && !cloudActiveForSelected);
  const startBlocked = adapter.mode === 'native' && (cloudEngine ? !runtime.cloudKeyConfigured || !state?.settings.cloudConsent || cloudBusyElsewhere : legacyEngine || !runtime.modelReady);
  const modelInstalled = runtime.modelInstalled ?? runtime.modelReady ?? false;
  const displayedInferenceState = cloudEngine ? (runtime.cloudKeyConfigured ? 'ready' : 'unloaded') : adapter.mode === 'native' && !runtime.modelReady && selected && ['idle', 'ready', 'unloaded'].includes(selected.inferenceState) ? runtime.modelState ?? 'unloaded' : selected?.inferenceState ?? 'idle';
  const inferenceStatusLabel = cloudEngine ? cloudActiveForSelected ? '云端实时转写' : cloudBusyElsewhere ? '另一门课程正在云端转写' : selected?.inferenceState === 'paused' ? '云端已暂停 · 录音继续保存在本机' : runtime.cloudKeyConfigured ? '云端已配置' : '配置云端转写' : legacyEngine ? '请选择实时转写引擎' : inferenceLabel[displayedInferenceState] || '等待转写';
  const recordingStatusLabel = selected?.recordingState === 'stopped' && selected.runs.length === 0 ? '准备开始录音' : selected ? recordingLabel[selected.recordingState] || selected.recordingState : '';
  const recordVisualState: 'ready' | 'starting' | 'recording' | 'paused' | 'stopping' | 'blocked' = captureActionPending === 'start' || selected?.recordingState === 'starting' ? 'starting' : captureActionPending === 'stop' || selected?.recordingState === 'stopping' ? 'stopping' : selected?.recordingState === 'paused' ? 'paused' : captureInProgress ? 'recording' : startBlocked ? 'blocked' : 'ready';
  const retrySelectedInference = async () => { if (!selected || cloudBusyElsewhere) return; await update(adapter.retryInference(selected.id)); };
  const selectEngine = async (engine: 'qwen' | 'soniox') => {
    if (!state || engineSwitchBlocked) return;
    setEngineOpen(false);
    if (engine === state.settings.engine) return;
    if (engine === 'soniox' && (!runtime.cloudKeyConfigured || !state.settings.cloudConsent)) { setPendingEngine('soniox'); setModal('settings'); return; }
    await update(adapter.dispatch({ type: 'settings', commandId: commandId(), settings: { ...state.settings, engine } }));
  };

  if (!state) return <main className="loading-screen"><div className="loading-mark">LE</div><p>正在打开课程…</p>{fatal && <p className="inline-error">{fatal}</p>}</main>;
  return <div className={`app-shell ${editor && !editor.exiting ? 'editor-open' : ''} ${fullEditor ? 'full-editing' : ''} ${sidebarExpanded ? 'sidebar-expanded' : 'sidebar-collapsed'}`} style={{ '--reader-font': `${fontSize}px` } as React.CSSProperties}>
    <CourseSidebar projects={state.projects ?? []} sessions={state.sessions} selectedId={state.selectedSessionId} expanded={sidebarExpanded} revealProjectId={sidebarRevealProjectId ?? selected?.projectId ?? null} onToggle={() => setSidebarExpanded(value => !value)} onSearch={() => setSearchOpen(true)} onSelect={setSelected} onNew={() => { setNewProjectId(null); setModal('new'); }} onNewProjectCourse={(projectId) => { setNewProjectId(projectId); setModal('new'); }} onCreateProject={createProject} onRenameProject={renameProject} onMove={moveSession} onRename={(sessionId, title) => { const next = title.trim(); if (next && next !== state.sessions.find((session) => session.id === sessionId)?.title) void update(adapter.dispatch({ type: 'renameSession', commandId: commandId(), sessionId, title: next })); }} onImport={() => void importCoursePackage()} />
    <GlassSurface className="workspace">
      <header className="topbar" aria-label="课程工具栏">
        <div className={`topbar-title ${introHidden ? 'is-visible' : ''}`} aria-hidden={!introHidden}>{selectedProject && <><span>{selectedProject.title}</span><ChevronRight size={12} strokeWidth={1.8} /></>}<strong>{selected?.title}</strong></div>
        <div className="toolbar-group reading-tools" aria-label="阅读工具">
          <IconButton label="搜索所有课堂" active={searchOpen} onClick={() => setSearchOpen(true)}><Search size={17} /></IconButton>
          {selected?.segments.length ? <button className={`follow-control ${viewMode === 'following' ? '' : 'is-paused'}`} aria-label={viewMode === 'following' ? `修订当前句，快捷键 ${shortcutLabel('E')}` : unseen ? `${unseen} 条新内容，回到实时` : '回到实时'} title={fullEditor ? '完成全文编辑后可修订单句' : viewMode === 'following' ? `修订当前句 · ${shortcutLabel('E')}` : '回到实时'} onClick={viewMode === 'following' ? editCurrentSentence : returnLatest} disabled={Boolean(editor || fullEditor)}>{viewMode === 'following' ? <><Pencil size={14} /> <span>修订当前句</span><kbd>{shortcutLabel('E')}</kbd></> : <><ChevronDown className="return-chevron" size={14} strokeWidth={1.9} /> <span>{unseen ? `${unseen} 条 · 回到实时` : '回到实时'}</span></>}</button> : null}
          <div className="font-control" role="group" aria-label="转写字号">{([[15, '小号字'], [17, '中号字'], [19, '大号字']] as const).map(([size, label]) => <button key={size} className={fontSize === size ? 'active' : ''} aria-label={label} title={label} aria-pressed={fontSize === size} onClick={() => setFontSize(size)}>A</button>)}</div>
        </div>
        <div className="toolbar-group model-tools" aria-label="转写设置">
          {pendingCount > 0 && <button className="pending-button" aria-label={`${pendingCount} 条待核对`} title={`${pendingCount} 条待核对`} onClick={() => { const first = selected?.segments.find((segment) => segment.pendingMachine); if (first) { setReviewSegment(first.id); setModal('review'); } }}><CircleAlert size={15} />{pendingCount}<span> 条待核对</span></button>}
          {cloudEngine && (cloudActiveForSelected || selected?.inferenceState === 'paused') && <button className="cloud-task-action" onClick={() => void toggleCloudProcessing()} disabled={cloudActionPending || cloudBusyElsewhere}>{cloudActionPending ? '正在更新…' : cloudActiveForSelected ? '暂停云端转写' : '继续云端转写'}</button>}
          {cloudEngine && cloudBusyElsewhere && <button className="cloud-task-action" disabled>另一课程转写中</button>}
          <div className="menu-wrap" ref={engineWrapRef}><button ref={engineButtonRef} className="engine-button" aria-label="选择转写引擎" aria-haspopup="menu" aria-expanded={engineOpen} disabled={engineSwitchBlocked} title={captureInProgress ? '结束录音后切换转写引擎' : ['downloading', 'verifying'].includes(runtime.modelDownload?.phase ?? 'idle') ? '模型准备完成后切换转写引擎' : '选择转写引擎'} onClick={() => { setEngineOpen((open) => !open); setExportOpen(false); setMoreOpen(false); }} onKeyDown={(event) => { if (event.key === 'ArrowDown') { event.preventDefault(); setEngineOpen(true); setExportOpen(false); setMoreOpen(false); requestAnimationFrame(() => engineMenuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus()); } }}>{cloudEngine ? <Cloud size={14} /> : <HardDrive size={14} />}<span>{cloudEngine ? 'Soniox' : legacyEngine ? '选择引擎' : 'Qwen'}</span><ChevronDown className={`control-chevron ${engineOpen ? 'is-open' : ''}`} size={13} strokeWidth={1.9} /></button>{engineOpen && <div ref={engineMenuRef} className="export-menu engine-menu" role="menu" onKeyDown={moveMenuFocus}><button role="menuitem" className={!cloudEngine && !legacyEngine ? 'selected' : ''} onClick={() => void selectEngine('qwen')}><span><strong>Qwen3-ASR</strong><small>在这台电脑上识别</small></span></button><button role="menuitem" className={cloudEngine ? 'selected' : ''} onClick={() => void selectEngine('soniox')}><span><strong>Soniox</strong><small>云端实时识别</small></span></button></div>}</div>
          <select className="source-select" value={source} onChange={(event) => setSource(event.target.value as typeof source)} disabled={captureInProgress} aria-label="音频来源"><option value="microphone">麦克风</option><option value="system" disabled={runtime.systemAudioAvailable === false}>系统声音{runtime.systemAudioAvailable === false ? '（不可用）' : ''}</option></select>
        </div>
        <div className="toolbar-group capture-tools" title={selected ? `${recordingStatusLabel} · ${inferenceStatusLabel}` : undefined}>
          {selected && ['recording', 'paused'].includes(selected.recordingState) && <button className={`record-pause-button ${selected.recordingState === 'paused' ? 'is-paused' : ''}`} onClick={() => void toggleRecordingPause()} disabled={Boolean(captureActionPending)} aria-label={selected.recordingState === 'paused' ? '继续录音' : '暂停录音'} title={selected.recordingState === 'paused' ? '继续录音' : '暂停录音'}>{captureActionPending === 'pause' ? <><Pause size={14} /> <span>暂停中…</span></> : captureActionPending === 'resume' ? <><Play size={14} fill="currentColor" /> <span>继续中…</span></> : selected.recordingState === 'paused' ? <><Play size={14} fill="currentColor" /> <span>继续录音</span></> : <><Pause size={14} fill="currentColor" /> <span>暂停录音</span></>}</button>}
          <button className={`record-button is-${recordVisualState}`} onClick={toggleRecording} aria-label={['recording', 'paused'].includes(recordVisualState) ? '停止录音' : undefined} disabled={!selected || Boolean(captureActionPending) || selected?.recordingState === 'stopping' || (!captureInProgress && startBlocked)}><RecordGlyph state={recordVisualState} />{['recording', 'paused'].includes(recordVisualState) && <><span className="record-timer">{fmtDuration(totalSamples)}</span><LevelMeter live={recordVisualState === 'recording'} /></>}<span className="record-label">{captureActionPending === 'stop' ? '正在结束…' : captureActionPending === 'start' ? cloudEngine ? '正在连接…' : '正在启动…' : captureInProgress ? selected?.recordingState === 'stopping' ? '正在结束…' : '停止录音' : adapter.mode === 'demo' ? '开始演示' : displayedInferenceState === 'loading' && !legacyEngine ? '正在准备…' : startBlocked ? legacyEngine ? '选择引擎' : cloudBusyElsewhere ? '云端忙碌' : cloudEngine ? '配置云端' : '模型未就绪' : '开始录音'}</span></button>
        </div>
        <div className="toolbar-group action-tools" aria-label="文件与设置">
          <div className="menu-wrap" ref={exportWrapRef}><button ref={exportButtonRef} className="export-button" aria-label="导出" aria-haspopup="menu" aria-expanded={exportOpen} title="导出" onClick={() => { setExportOpen((open) => !open); setEngineOpen(false); setMoreOpen(false); }} onKeyDown={(event) => { if (event.key === 'ArrowDown') { event.preventDefault(); setExportOpen(true); setEngineOpen(false); setMoreOpen(false); requestAnimationFrame(() => exportMenuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus()); } }}><Download size={15} /><span>导出</span><ChevronDown className={`control-chevron ${exportOpen ? 'is-open' : ''}`} size={13} strokeWidth={1.9} /></button>{exportOpen && <div ref={exportMenuRef} className="export-menu" role="menu" onKeyDown={moveMenuFocus}>{([['markdown', 'Markdown', FileText], ['html', '课程阅读 HTML', BookOpen], ['pdf', '精排 PDF', FileText], ['wav', 'WAV 音频', FileAudio], ['lecture', '原生课程包', ArchiveRestore]] as const).map(([format, label, Icon]) => <button role="menuitem" key={format} disabled={format === 'pdf' && !canExportNativePdf(runtime.platform)} title={format === 'pdf' && !canExportNativePdf(runtime.platform) ? PDF_PLATFORM_NOTICE : undefined} onClick={() => { setExportOpen(false); if (selected) void saveFullDocument().then((saved) => { if (saved) return adapter.exportSession(selected.id, format); }).catch(showError); }}><Icon size={16} /><span><strong>{label}</strong><small>{format === 'lecture' ? '可再次导入 LectureEdit' : format === 'wav' ? '导出原始课程录音' : format === 'pdf' ? (canExportNativePdf(runtime.platform) ? '适合打印与阅读的版式' : 'macOS 功能 · Windows 可导出 HTML 后打印') : '包含转写和课堂资料'}</small></span></button>)}</div>}</div>
          <div className="menu-wrap" ref={moreWrapRef}><button ref={moreButtonRef} className={`icon-button ${moreOpen ? 'is-active' : ''}`} aria-label="更多操作" aria-haspopup="dialog" aria-expanded={moreOpen} title="更多操作" onClick={() => { setMoreOpen((open) => !open); setEngineOpen(false); setExportOpen(false); }}><MoreHorizontal size={18} /></button>{moreOpen && selected && <div className="export-menu compact-menu" role="dialog" aria-label="更多操作">
            <label className="responsive-menu-control compact-source-control"><Mic size={16} /><span><strong>音频来源</strong><select value={source} onChange={(event) => { setSource(event.target.value as typeof source); setMoreOpen(false); }} disabled={captureInProgress} aria-label="音频来源"><option value="microphone">麦克风</option><option value="system" disabled={runtime.systemAudioAvailable === false}>系统声音{runtime.systemAudioAvailable === false ? '（不可用）' : ''}</option></select></span></label>
            <div className="responsive-menu-control compact-font-control"><span>转写字号</span><div role="group" aria-label="转写字号">{([[15, '小', '小号字'], [17, '中', '中号字'], [19, '大', '大号字']] as const).map(([size, text, label]) => <button key={size} className={fontSize === size ? 'active' : ''} aria-label={label} aria-pressed={fontSize === size} onClick={() => setFontSize(size)}>{text}</button>)}</div></div>
            <button onClick={() => { setMoreOpen(false); void update(adapter.importAudio(selected.id)); }}><FileAudio size={16} /><span><strong>导入 WAV</strong><small>添加已有录音</small></span></button><button disabled={cloudBusyElsewhere} onClick={() => { setMoreOpen(false); void retrySelectedInference(); }}><ArchiveRestore size={16} /><span><strong>{cloudBusyElsewhere ? '另一课程正在转写' : '重试转写'}</strong><small>{cloudBusyElsewhere ? '完成后可重试当前课程' : '重新启动处理'}</small></span></button>
          </div>}</div>
          <IconButton label="设置" onClick={() => { setPendingEngine(undefined); setModal('settings'); }}><SettingsIcon size={18} /></IconButton>
        </div>
      </header>
      {fatal && <FloatingPortal><div className="error-banner" role="alert"><CircleAlert size={16} /><span>{fatal}</span><button onClick={dismissFatal} aria-label="关闭错误提示"><X size={15} /></button></div></FloatingPortal>}
      {notice && <div className="toast" role="status"><Check size={15} />{notice}</div>}
      <LayoutGroup id="transcript-edit"><div className="content-frame">
        <AnimatePresence initial={false}>{editor && activeSegment && <CorrectionPanel draft={editor.draft} segment={activeSegment} text={editor.text} status={saveStatus} commitError={commitError} onReview={() => { setReviewSegment(activeSegment.id); setModal('review'); }} originRect={editor.originRect} exiting={editor.exiting} onText={changeDraft} onClose={() => void closeEditor()} onDiscard={() => void discardEditor()} onCommit={() => void commitEditor()} onUndo={undoDraft} onRedo={redoDraft} onPlay={() => void playSegment(activeSegment)} />}</AnimatePresence>
        <main className="document-pane">
          {selected && (selected.error || selected.recordingState === 'error' || selected.inferenceState === 'error') && <div className={`session-error ${fatal ? 'is-offset' : ''}`} role="alert"><CircleAlert size={16} /><span>{selected.error || (selected.recordingState === 'error' ? recordingLabel.error : inferenceLabel.error)}</span></div>}
          {!selected ? <div className="empty-state"><BookOpen size={32} /><h2>开始第一节课堂</h2><p>新建课堂后即可录音、转写，并把实时内容整理成课堂记录。</p><button className="primary-button" onClick={() => setModal('new')}><Plus size={16} /> 新建课堂</button></div> : selected.segments.length === 0 ? <EmptyTranscriptState session={selected} samples={totalSamples} demo={adapter.mode === 'demo'} cloud={cloudEngine} cloudReady={Boolean(runtime.cloudKeyConfigured && state.settings.cloudConsent)} legacyEngine={legacyEngine} retryDisabled={cloudBusyElsewhere} runtime={runtime} onStart={() => void toggleRecording()} onImport={() => void update(adapter.importAudio(selected.id))} onRetry={() => void retrySelectedInference()} onPrepare={() => void prepareModel()} onSetup={() => { setPendingEngine(undefined); setModal('settings'); }} /> : <div key={selected.id} className={`transcript-scroll is-${viewMode}`} ref={transcriptRef} tabIndex={0} aria-label="课堂转写" onWheel={(event) => { if (event.deltaY < 0) pauseFollowing(); }} onScroll={handleTranscriptScroll} onKeyDown={handleTranscriptKeyDown} onPointerDown={() => { pointerDownRef.current = true; }} onPointerUp={() => { pointerDownRef.current = false; }} onPointerLeave={() => { pointerDownRef.current = false; }} onPointerCancel={() => { pointerDownRef.current = false; }} onTouchStart={handleTranscriptTouchStart} onTouchMove={handleTranscriptTouchMove} onTouchEnd={() => { touchStartYRef.current = null; }} onTouchCancel={() => { touchStartYRef.current = null; }}>
            <div className="transcript-column" ref={transcriptContentRef}>
              {fullEditor && fullEditor.sessionId === selected.id ? <FullTranscriptEditor session={selected} values={fullEditor.values} original={fullEditor.original} status={fullEditor.status} error={fullEditor.error} onText={(segmentId, text) => setFullEditor((value) => value ? { ...value, values: { ...value.values, [segmentId]: text }, status: 'editing', error: '' } : value)} onSave={() => void saveFullDocument()} onDone={() => void saveFullDocument(true)} /> :
              <TranscriptAssist segments={selected.segments} sessionId={selected.id} configured={Boolean(runtime.deepseekKeyConfigured)} onSave={async (segmentId, text, sourceLabel) => Boolean(await update(adapter.dispatch({ type: 'addNote', commandId: commandId(), sessionId: selected.id, segmentId, kind: 'note', text, sourceLabel, imageData: null })))}>
              <div className="session-intro"><div>{selectedProject && <p className="session-crumb"><Folder size={13} strokeWidth={1.8} /><span>{selectedProject.title}</span></p>}<div className="session-title-row"><h2>{selected.title}</h2>{adapter.mode === 'demo' && <span className="demo-label">演示</span>}</div><p className="session-meta"><span>{new Intl.DateTimeFormat('zh-CN', { weekday: 'long', month: 'long', day: 'numeric' }).format(selected.createdAt)}</span>{totalSamples > 0 && <span className="meta-number">{fmtDuration(totalSamples)}</span>}<span className="meta-number">{wordCount.toLocaleString('zh-CN')} 字</span><span>{cloudEngine ? 'Soniox · stt-rt-v5' : selected.runs[0]?.source === 'import' ? '导入音频 · 本地转写' : selected.runs[0]?.source === 'system' ? '系统声音 · 本地转写' : '麦克风 · 本地转写'}</span><span className="saved-state"><Check size={13} /> 已保存</span></p></div><button className="full-edit-trigger" onClick={() => void beginFullEdit()} disabled={captureInProgress} title={captureInProgress ? '录音结束后编辑全文' : '在连续页面中编辑完整文稿'}><Pencil size={14} /> 编辑全文</button></div>
              {transcriptGroups.map((group, groupIndex) => <section className={`transcript-paragraph ${groupIndex >= transcriptGroups.length - 3 ? 'is-recent' : ''}`} key={group.map((segment) => segment.id).join('|')}><button className="paragraph-time" tabIndex={-1} aria-label={`从 ${fmtTime(segmentClock(selected, group[0]))} 播放`} title="从这里播放" onClick={() => void playSegment(group[0])}>{fmtTime(segmentClock(selected, group[0]))}</button><p>{group.map((segment, index) => {
                const entering = seenSessionRef.current === selected.id && seenSegmentsRef.current.size > 0 && !seenSegmentsRef.current.has(segment.id);
                return <Fragment key={segment.id}>{index > 0 ? ' ' : null}<TranscriptBlock segment={segment} active={editor?.draft.segmentId === segment.id} playingKey={playingSegmentId} entering={entering} query="" onEdit={() => void beginEdit(segment)} onPlay={(anchor, playbackKey) => void playSegment(segment, anchor, playbackKey)} onUndo={() => void moveSegmentHistory(segment, 'undo')} onRedo={() => void moveSegmentHistory(segment, 'redo')} onReview={() => { setReviewSegment(segment.id); setModal('review'); }} composer={(sliceKey) => <InlineNoteComposer open={noteSegment === sliceKey} onOpenChange={(open) => { setNoteSegment(open ? sliceKey : null); if (open) setViewMode('reading_history'); }} kind={noteKind} setKind={setNoteKind} configured={Boolean(runtime.deepseekKeyConfigured)} onSubmit={async (value) => Boolean(await update(adapter.dispatch({ type: 'addNote', commandId: commandId(), sessionId: selected.id, segmentId: segment.id, ...value })))} />} /></Fragment>;
              })}</p>{group.flatMap((segment) => selected.notes.filter((note) => note.segmentId === segment.id)).map((note) => <NoteBlock key={note.id} note={note} onRemove={() => void update(adapter.dispatch({ type: 'removeNote', commandId: commandId(), sessionId: selected.id, noteId: note.id }))} />)}</section>)}
              <div className="transcript-follow-anchor" ref={followAnchorRef} aria-hidden="true" />
              <div className="transcript-end"><span />{selected.recordingState === 'recording' ? <p><span className="live-caret" />等待下一段内容</p> : <p>转写结束{totalSamples > 0 && <> · {fmtDuration(totalSamples)}</>}</p>}<span /></div>
              </TranscriptAssist>}
            </div>
          </div>}
        </main>
      </div></LayoutGroup>
    </GlassSurface>
    <GlobalSearch open={searchOpen} sessions={state.sessions} projects={state.projects ?? []} onClose={() => setSearchOpen(false)} onNavigate={navigateSearch} />
    {modal === 'new' && <Dialog title="新建课堂" description={newProjectId ? `课堂将收入课程分组“${state.projects.find(project => project.id === newProjectId)?.title ?? '未命名课程'}”。` : '课堂创建成功后，再在准备好时开始录音。'} onClose={() => { setModal(null); setNewProjectId(null); }}><label className="form-field"><span>课堂名称</span><input autoFocus value={newTitle} onChange={(e) => setNewTitle(e.target.value)} onKeyDown={(e) => { if (e.key === 'Enter' && !e.nativeEvent.isComposing) void createCourse(); }} placeholder="例如：Lecture 1 · Markets & Choices" /></label>{dialogError && <p className="inline-error" role="alert"><CircleAlert size={14} />{dialogError}</p>}<div className="course-setup"><div><Languages size={17} /><span><strong>{state.settings.language === 'auto' ? '自动检测语言' : state.settings.language === 'zh' ? '中文' : state.settings.language === 'en' ? '英语' : state.settings.language}</strong><small>可在本地转写设置中修改</small></span></div><div><Mic size={17} /><span><strong>{source === 'microphone' ? '麦克风' : '系统声音'}</strong><small>开始录音前可以切换音频来源</small></span></div></div><footer><button className="secondary-button" onClick={() => { setModal(null); setNewProjectId(null); }}>取消</button><button className="primary-button" disabled={creatingCourse} onClick={() => void createCourse()}>{creatingCourse ? '正在创建…' : '创建课堂'}</button></footer></Dialog>}
    {modal === 'settings' && <SettingsDialog key={selected?.id ?? 'none'} settings={state.settings} session={selected} project={selectedProject} runtime={runtime} initialEngine={pendingEngine} onClose={() => { setPendingEngine(undefined); setModal(null); }} onPick={handlePickPath} onInstall={async () => { const result = await update(adapter.dispatch({ type: 'installModel', commandId: commandId(), engine: 'qwen' }), { background: true }); if (!result) return false; return prepareModel(); }} onPrepare={prepareModel} onSave={async (settings, vocabulary) => { let failure = ''; const result = await update(adapter.dispatch({ type: 'settings', commandId: commandId(), settings, ...(selectedProject ? { projectId: selectedProject.id, projectVocabulary: vocabulary.projectVocabulary } : {}), ...(selected ? { sessionId: selected.id, sessionVocabulary: vocabulary.sessionVocabulary } : {}) }), { onError: (message) => { failure = message; } }); if (result) { setPendingEngine(undefined); setModal(null); return true; } if (failure) throw new Error(failure); return false; }} />}
    {modal === 'review' && reviewed && <ReviewDialog segment={reviewed} error={dialogError} onClose={() => setModal(null)} onResolve={(action, text) => { if (!selected) return; void update(adapter.dispatch({ type: 'resolve', commandId: commandId(), sessionId: selected.id, segmentId: reviewed.id, expectedUserSeq: reviewed.userSeq, expectedMachineRevision: reviewed.machineRevision, action, text }), { onError: setDialogError }).then((result) => { if (result) setModal(null); }); }} />}
  </div>;
}
