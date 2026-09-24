import { useEffect, useRef, useState, type ReactNode } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'motion/react';
import { ArrowLeft, ArrowRight, FolderPlus, HardDrive, Mic, Pencil, Search, ShieldCheck, StickyNote } from 'lucide-react';
import { shortcutLabel } from './platform';

const DONE_KEY = 'suitang.onboarding-done';

/** First run only: no record of the guide and an empty library. An existing library means the
 *  person already knows the app, so it is marked as seen without being shown. Storage that
 *  cannot be read never shows the guide, so it cannot come back on every launch. */
export function shouldShowOnboarding(hasSessions: boolean): boolean {
  try {
    if (localStorage.getItem(DONE_KEY)) return false;
    if (hasSessions) { localStorage.setItem(DONE_KEY, '1'); return false; }
    return true;
  } catch { return false; }
}

export function markOnboardingDone() {
  try { localStorage.setItem(DONE_KEY, '1'); } catch { /* storage unavailable */ }
}

type Step = { icon: ReactNode; title: string; body: ReactNode; keys?: string[] };

const steps = (): Step[] => [
  { icon: <img src="/app-icon.png" alt="" width={64} height={64} />, title: '欢迎使用随堂', body: '边录音，边改转写，边补充课堂笔记。花一分钟看看它怎么用，之后随时可以在设置里重新打开这份引导。' },
  { icon: <HardDrive size={30} strokeWidth={1.6} />, title: '先准备本地模型', body: <>打开右上角的<b>设置</b>，在“课堂转写”里选择<b>本地</b>并下载模型。约 1 GB，只需下载一次，之后转写都在这台电脑上完成。</> },
  { icon: <FolderPlus size={30} strokeWidth={1.6} />, title: '按课程整理课堂', body: <>在左侧为每门课<b>新建课程</b>，每次上课再<b>新建课堂</b>。名称和日期随时可以改，课堂也能移到别的课程里。</> },
  { icon: <Mic size={30} strokeWidth={1.6} />, title: '开始录音', body: <>现场上课选<b>麦克风</b>，看网课选<b>系统声音</b>，然后按录音键。转写会实时出现在正文里。录音前请先取得老师和同学的同意。</> },
  { icon: <Pencil size={30} strokeWidth={1.6} />, title: '听错了，当场改', body: <>点一下任何一句，选<b>修订</b>；或者按快捷键修订选中的那句（没选中时改最新一句）。你修改时，新内容照常转写。</>, keys: [shortcutLabel('E')] },
  { icon: <StickyNote size={30} strokeWidth={1.6} />, title: '在讲到的地方记笔记', body: <>点一句后选<b>笔记</b>，可以补充文字、例子、LaTeX 公式，或直接粘贴课件截图。笔记会留在对应的位置，和录音时间对应。</> },
  { icon: <Search size={30} strokeWidth={1.6} />, title: '课后搜索与回放', body: <>搜索所有课堂的转写和笔记，结果可以直接定位到原文。点句子旁的<b>回放</b>，就能复听老师当时怎么说。</>, keys: [shortcutLabel('K')] },
  { icon: <ShieldCheck size={30} strokeWidth={1.6} />, title: '导出，也放心', body: <>从<b>导出</b>菜单保存为 PDF、Markdown、网页或课程包。录音和转写默认只留在本机；DeepSeek、Soniox 等云端功能要你在设置里主动开启才会使用。</> },
];

export function Onboarding({ onClose }: { onClose: () => void }) {
  const list = useRef(steps()).current;
  const [[index, direction], setPage] = useState<[number, number]>([0, 0]);
  const reduceMotion = useReducedMotion();
  const primary = useRef<HTMLButtonElement>(null);
  const last = index === list.length - 1;
  const step = list[index];

  const go = (next: number) => { if (next >= 0 && next < list.length && next !== index) setPage([next, next > index ? 1 : -1]); };
  const finish = () => { markOnboardingDone(); onClose(); };

  useEffect(() => { primary.current?.focus({ preventScroll: true }); }, [index]);
  const dialog = useRef<HTMLElement>(null);
  useEffect(() => {
    // Capture on window so the app's own shortcuts (⌘E, ⌘K…) never act behind the guide.
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      event.stopPropagation();
      if (event.key === 'Escape') { event.preventDefault(); finish(); }
      else if (event.key === 'ArrowRight') { event.preventDefault(); if (last) finish(); else go(index + 1); }
      else if (event.key === 'ArrowLeft') { event.preventDefault(); go(index - 1); }
      else if (event.key === 'Tab') {
        const items = [...(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled)') ?? [])];
        const current = items.indexOf(document.activeElement as HTMLElement);
        const next = items.at((current + (event.shiftKey ? -1 : 1)) % items.length);
        event.preventDefault(); next?.focus();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  const offset = reduceMotion ? 0 : 28;
  const spring = { type: 'spring' as const, bounce: 0, duration: 0.42 };

  return <motion.div className="onboarding-layer" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.2 }}>
    <motion.section ref={dialog} className="onboarding" role="dialog" aria-modal="true" aria-labelledby="onboarding-title" aria-describedby="onboarding-body"
      initial={reduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.96, y: 8 }} animate={{ opacity: 1, scale: 1, y: 0 }} exit={reduceMotion ? { opacity: 0 } : { opacity: 0, scale: 0.97 }} transition={spring}>
      {!last && <button className="onboarding-skip" onClick={finish}>跳过</button>}
      <div className="onboarding-stage">
        <AnimatePresence initial={false} custom={direction} mode="popLayout">
          <motion.div key={index} className="onboarding-page" custom={direction}
            variants={{ enter: (d: number) => ({ opacity: 0, x: d * offset }), center: { opacity: 1, x: 0 }, leave: (d: number) => ({ opacity: 0, x: -d * offset }) }}
            initial="enter" animate="center" exit="leave" transition={spring}>
            <div className="onboarding-hero" aria-hidden="true">
              <div className={`onboarding-icon ${index === 0 ? 'is-app' : ''}`}>{step.icon}</div>
              {step.keys && <div className="onboarding-keys">{step.keys.map((key) => <kbd key={key}>{key}</kbd>)}</div>}
            </div>
            <p className="onboarding-count">{index + 1} / {list.length}</p>
            <h2 id="onboarding-title">{step.title}</h2>
            <p id="onboarding-body" className="onboarding-body">{step.body}</p>
          </motion.div>
        </AnimatePresence>
      </div>
      <footer>
        <div className="onboarding-dots" role="group" aria-label="引导进度">
          {list.map((item, dot) => <button key={item.title} className={dot === index ? 'is-current' : ''} aria-label={`第 ${dot + 1} 步：${item.title}`} aria-current={dot === index ? 'step' : undefined} onClick={() => go(dot)} />)}
        </div>
        <div className="onboarding-actions">
          {index > 0 && <button className="onboarding-back" onClick={() => go(index - 1)} aria-label="上一步"><ArrowLeft size={15} /></button>}
          <button ref={primary} className="primary-button onboarding-next" onClick={() => last ? finish() : go(index + 1)}>{last ? '开始使用' : index === 0 ? '开始了解' : '下一步'}{!last && <ArrowRight size={15} />}</button>
        </div>
      </footer>
    </motion.section>
  </motion.div>;
}
