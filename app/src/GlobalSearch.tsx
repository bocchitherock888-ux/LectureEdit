import { shortcutLabel } from './platform';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { FloatingFocusManager, FloatingOverlay, FloatingPortal, useDismiss, useFloating, useInteractions, useRole } from '@floating-ui/react';
import { ArrowUpRight, FileText, Folder, Play, Search, X } from 'lucide-react';
import type { Project, Session } from './types';
import { searchExcerpt, searchLibrary, type SearchResult } from './search';
import './global-search.css';

export type { SearchResult } from './search';

const time = (ms: number) => {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60).toString().padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`;
};

function MarkedText({ text, query }: { text: string; query: string }) {
  const terms = query.trim().split(/\s+/u).filter(Boolean).map(term => term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  if (!terms.length) return <>{text}</>;
  const expression = new RegExp(`(${terms.join('|')})`, 'giu');
  return <>{text.split(expression).map((part, index) => index % 2 ? <mark key={index}>{part}</mark> : part)}</>;
}

export function GlobalSearch({ open, sessions, projects, onClose, onNavigate }: {
  open: boolean;
  sessions: Session[];
  projects: Project[];
  onClose: () => void;
  onNavigate: (result: SearchResult, play: boolean) => Promise<boolean>;
}) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const resultsId = useId();
  const { refs, context } = useFloating({ open, onOpenChange: value => { if (!value && !busy) onClose(); } });
  const dismiss = useDismiss(context, { enabled: !busy, outsidePressEvent: 'mousedown' });
  const role = useRole(context, { role: 'dialog' });
  const { getFloatingProps } = useInteractions([dismiss, role]);
  const results = useMemo(() => searchLibrary(sessions, projects, query), [sessions, projects, query]);
  const visible = results.slice(0, 100);
  const selectedIndex = Math.min(active, Math.max(0, visible.length - 1));
  const busyRef = useRef(false);
  useEffect(() => { if (open) { setQuery(''); setActive(0); } }, [open]);
  useEffect(() => {
    refs.floating.current?.querySelector<HTMLElement>(`[data-search-index="${selectedIndex}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [selectedIndex, refs.floating]);
  const navigate = async (result: SearchResult, play: boolean) => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true);
    try { if (await onNavigate(result, play)) onClose(); }
    finally { busyRef.current = false; setBusy(false); }
  };
  if (!open) return null;
  return <FloatingPortal><FloatingOverlay className="library-search-overlay" lockScroll>
    <FloatingFocusManager context={context} initialFocus={input}>
      <section ref={refs.setFloating} {...getFloatingProps()} className="library-search" aria-label="搜索所有课堂">
        <div className="library-search-input">
          <Search size={21} aria-hidden="true" />
          <input ref={input} role="combobox" aria-haspopup="grid" aria-expanded={visible.length > 0} aria-controls={resultsId} aria-activedescendant={visible.length ? `${resultsId}-${selectedIndex}` : undefined} aria-autocomplete="list" aria-label="搜索转写、笔记或课程" placeholder="搜索转写、笔记或课程…" value={query} onChange={event => { setQuery(event.target.value); setActive(0); }} onKeyDown={event => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
              event.preventDefault(); setActive(index => Math.min(Math.max(0, visible.length - 1), Math.max(0, index + (event.key === 'ArrowDown' ? 1 : -1))));
            }
            if (event.key === 'Enter' && visible[selectedIndex]) { event.preventDefault(); void navigate(visible[selectedIndex], event.altKey); }
          }} />
          <button aria-label="关闭搜索" disabled={busy} onClick={onClose}><X size={18} /></button>
        </div>
        <div className="library-search-summary" role="status">{query.trim() ? `找到 ${results.length} 处${results.length > 100 ? ' · 显示前 100 处，请增加关键词缩小范围' : ''}` : '所有课程 · 转写与课堂资料'}</div>
        <div id={resultsId} role={visible.length ? 'grid' : undefined} className="library-search-results" aria-label="搜索结果" aria-busy={busy}>
          {!query.trim() ? <div className="library-search-empty"><Search size={28} /><h3>找回课堂上讲过的内容</h3><p>试着输入 opportunity cost、demand curve，或课程名称。</p></div> : !results.length ? <div className="library-search-empty"><h3>没有找到相关内容</h3><p>换一个关键词，或试试英文课堂中的原词。</p></div> : visible.map((result, index) => <div key={result.id} id={`${resultsId}-${index}`} role="row" aria-selected={index === selectedIndex} data-search-index={index} className={`library-search-result ${index === selectedIndex ? 'is-active' : ''}`} onMouseEnter={() => setActive(index)}>
            <div role="gridcell" className="library-search-main-cell"><button className="library-search-open" disabled={busy} onFocus={() => setActive(index)} onClick={() => void navigate(result, false)}>
              <span className="library-search-location">{result.projectTitle && <span><Folder size={12} />{result.projectTitle}</span>}<span>{result.title}</span><time>{new Intl.DateTimeFormat('zh-CN', { month: 'short', day: 'numeric' }).format(result.createdAt)}</time></span>
              <span className="library-search-excerpt"><MarkedText text={searchExcerpt(result.text, query)} query={query} /></span>
              <span className="library-search-anchor">{result.kind === 'note' ? '课堂资料' : result.kind === 'title' ? <><FileText size={12} />课堂记录</> : '转写'}{result.startMs !== null && <> · {time(result.startMs)}</>}<ArrowUpRight size={13} /></span>
            </button></div>
            {result.segmentId && <div role="gridcell"><button className="library-search-play" disabled={busy} onClick={() => void navigate(result, true)} aria-label={`回放 ${result.title} ${time(result.startMs ?? 0)}`} title="定位并回放"><Play size={15} fill="currentColor" /></button></div>}
          </div>)}
        </div>
        <footer><span>↑ ↓ 选择</span><span>↵ 定位原文</span><span>{shortcutLabel('Enter', undefined, 'alt')} 回放</span><span>Esc 关闭</span></footer>
      </section>
    </FloatingFocusManager>
  </FloatingOverlay></FloatingPortal>;
}
