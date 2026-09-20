import { useEffect, useRef, useState } from 'react';
import { CircleAlert, Cloud, FolderOpen, HardDrive, KeyRound, Monitor, Moon, Sun, TextQuote, X } from 'lucide-react';
import { adapter } from './bridge';
import { DeepSeekConnection } from './DeepSeekTools';
import type { Project, RuntimeInfo, Session, Settings } from './types';
import './settings.css';

export type SettingsDialogProps = {
  settings: Settings;
  session: Session | null;
  project: Project | null;
  runtime: RuntimeInfo;
  initialEngine?: 'qwen' | 'soniox';
  onSave: (value: Settings, vocabulary: { projectVocabulary: string[]; sessionVocabulary: string[] }) => Promise<boolean>;
  onPick: (key: 'executable' | 'modelPath' | 'mmprojPath') => Promise<string | null>;
  onInstall: () => Promise<boolean>;
  onPrepare: () => Promise<boolean>;
  onClose: () => void;
};

export function SettingsDialog({ settings, session, project, runtime, initialEngine, onSave, onPick, onInstall, onPrepare, onClose }: SettingsDialogProps) {
  const [value, setValue] = useState(() => ({ ...settings, engine: initialEngine ?? settings.engine, customVocabulary: settings.customVocabulary ?? [], autoPolish: settings.autoPolish ?? false, theme: settings.theme ?? 'system' }));
  const [vocabularyScope, setVocabularyScope] = useState<'global' | 'project' | 'session'>(() => session ? 'session' : project ? 'project' : 'global');
  const [projectVocabulary, setProjectVocabulary] = useState(project?.customVocabulary ?? []);
  const [sessionVocabulary, setSessionVocabulary] = useState(session?.customVocabulary ?? []);
  const [installState, setInstallState] = useState<'idle' | 'downloading' | 'done' | 'error'>('idle');
  const [prepareState, setPrepareState] = useState<'idle' | 'loading' | 'error'>('idle');
  const [apiKey, setApiKey] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [keyConfigured, setKeyConfigured] = useState(Boolean(runtime.cloudKeyConfigured));
  const [deepseekConfigured, setDeepseekConfigured] = useState(Boolean(runtime.deepseekKeyConfigured));
  const savedPaths = useRef([settings.engine, settings.executable, settings.modelPath, settings.mmprojPath, JSON.stringify(settings.customVocabulary ?? []), String(settings.autoPolish ?? false), settings.theme ?? 'system'].join('\0'));
  const panel = useRef<HTMLElement>(null);
  const cloud = value.engine === 'soniox';
  const download = runtime.modelDownload;
  const downloading = installState === 'downloading' || download?.phase === 'downloading' || download?.phase === 'verifying';
  const downloadPercent = download?.totalBytes ? Math.min(100, Math.round(download.downloadedBytes / download.totalBytes * 100)) : 0;
  useEffect(() => setKeyConfigured(Boolean(runtime.cloudKeyConfigured)), [runtime.cloudKeyConfigured]);
  useEffect(() => setDeepseekConfigured(Boolean(runtime.deepseekKeyConfigured)), [runtime.deepseekKeyConfigured]);
  useEffect(() => {
    const next = [settings.engine, settings.executable, settings.modelPath, settings.mmprojPath, JSON.stringify(settings.customVocabulary ?? []), String(settings.autoPolish ?? false), settings.theme ?? 'system'].join('\0');
    if (savedPaths.current === next) return;
    savedPaths.current = next;
    setValue((current) => ({ ...current, engine: settings.engine, executable: settings.executable, modelPath: settings.modelPath, mmprojPath: settings.mmprojPath, customVocabulary: settings.customVocabulary ?? [], autoPolish: settings.autoPolish ?? false, theme: settings.theme ?? 'system' }));
  }, [settings.engine, settings.executable, settings.modelPath, settings.mmprojPath, settings.customVocabulary, settings.autoPolish, settings.theme]);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.focus();
    return () => previous?.focus();
  }, []);
  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.isComposing && !saving) onClose();
      if (event.key !== 'Tab') return;
      const controls = [...(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select, summary, a[href]') ?? [])].filter((item) => item.getClientRects().length > 0);
      const first = controls[0]; const last = controls.at(-1);
      if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel.current)) { event.preventDefault(); first?.focus(); }
    };
    window.addEventListener('keydown', close);
    return () => window.removeEventListener('keydown', close);
  }, [onClose, saving]);

  const pathField = (key: 'executable' | 'modelPath' | 'mmprojPath', label: string, placeholder: string) => (
    <label className="path-field"><span>{label}</span><div>
      <input value={value[key]} onChange={(event) => setValue({ ...value, [key]: event.target.value })} placeholder={placeholder} />
      <button onClick={async () => { const path = await onPick(key); if (path) setValue((current) => ({ ...current, [key]: path })); }} aria-label={`选择${label}`}><FolderOpen size={16} /></button>
    </div></label>
  );
  const installed = runtime.localModelInstalled ?? (settings.engine !== 'soniox' && (runtime.modelInstalled ?? runtime.modelReady ?? false));
  const sameEngine = (['engine', 'executable', 'modelPath', 'mmprojPath', 'language'] as const).every((key) => value[key] === settings[key]);
  const modelState = prepareState === 'loading' ? 'loading' : runtime.modelState ?? (runtime.modelReady ? 'ready' : 'unloaded');
  const healthTitle = value.engine === 'whisper' ? '请选择实时转写引擎' : !sameEngine ? '保存后准备所选模型' : !installed ? '需要下载本地模型' : modelState === 'ready' ? '本地模型已就绪' : modelState === 'loading' ? '正在加载模型' : modelState === 'error' || prepareState === 'error' ? '模型准备失败' : '等待加载模型';
  const normalizeVocabulary = (entries: string[]) => {
    const vocabulary = entries.map((entry) => entry.trim()).filter(Boolean);
    if (vocabulary.length > 200 || vocabulary.some((entry) => new TextEncoder().encode(entry).byteLength > 200) || new TextEncoder().encode(vocabulary.join('\n')).byteLength > 10000) throw new Error('每个词表最多 200 条，每条不超过 200 字节，总计不超过 10 KB。');
    return [...new Map(vocabulary.map((entry) => [entry.toLocaleLowerCase(), entry])).values()];
  };
  const visibleVocabulary = vocabularyScope === 'global' ? (value.customVocabulary ?? []) : vocabularyScope === 'project' ? projectVocabulary : sessionVocabulary;
  const changeVisibleVocabulary = (entries: string[]) => {
    if (vocabularyScope === 'global') setValue({ ...value, customVocabulary: entries });
    else if (vocabularyScope === 'project') setProjectVocabulary(entries);
    else setSessionVocabulary(entries);
  };
  const save = async () => {
    setError(''); setSaving(true);
    try {
      const customVocabulary = normalizeVocabulary(value.customVocabulary ?? []);
      const normalizedProjectVocabulary = normalizeVocabulary(projectVocabulary);
      const normalizedSessionVocabulary = normalizeVocabulary(sessionVocabulary);
      if (value.autoPolish && !deepseekConfigured) throw new Error('请先保存 DeepSeek API Key，再启用转写轻度整理。');
      if (cloud) {
        if (!value.cloudConsent) throw new Error('请先确认云端音频发送与账户费用。');
        if (adapter.mode === 'demo') throw new Error('请在桌面应用中配置 Soniox 连接。');
        if (apiKey.trim()) {
          await adapter.dispatch({ type: 'configureSoniox', apiKey: apiKey.trim() });
          setApiKey(''); setKeyConfigured(true);
        } else if (!keyConfigured) throw new Error('请输入具有实时语音识别权限的 Soniox API Key。');
      }
      if (!await onSave({ ...value, customVocabulary }, { projectVocabulary: normalizedProjectVocabulary, sessionVocabulary: normalizedSessionVocabulary })) setError('设置保存失败，请检查当前录音或转写状态后重试。');
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setSaving(false); }
  };

  return <div className="dialog-layer" role="presentation" onMouseDown={(event) => { if (event.currentTarget === event.target && !saving) onClose(); }}>
    <section ref={panel} tabIndex={-1} className="dialog transcription-settings" role="dialog" aria-modal="true" aria-label="设置">
      <header><div><h2>设置</h2><p>调整外观、课堂转写和智能服务。</p></div><button className="icon-button" aria-label="关闭" disabled={saving} onClick={onClose}><X size={18} /></button></header>
      <div className="settings-scroll">
      <fieldset className="appearance-settings"><legend>外观</legend><div className="appearance-choice" role="radiogroup" aria-label="界面外观">
        {([['system', '跟随系统', Monitor], ['light', '浅色', Sun], ['dark', '深色', Moon]] as const).map(([theme, label, Icon]) => <label className={(value.theme ?? 'system') === theme ? 'selected' : ''} key={theme}><input type="radio" name="appearance" value={theme} checked={(value.theme ?? 'system') === theme} disabled={saving} onChange={() => setValue({ ...value, theme })} /><Icon size={16} /><span>{label}</span></label>)}
      </div></fieldset>
      <div className="settings-section-label">课堂转写</div>
      <div className="engine-choice" role="radiogroup" aria-label="转写方式">
        <label className={cloud ? '' : 'selected'}><input type="radio" name="transcription-engine" value="qwen" checked={value.engine === 'qwen'} disabled={saving || downloading} onChange={() => { setValue({ ...value, engine: 'qwen' }); setError(''); }} /><HardDrive size={18} /><span><strong>本地</strong><small>Qwen3-ASR</small></span></label>
        <label className={cloud ? 'selected' : ''}><input type="radio" name="transcription-engine" value="soniox" checked={cloud} disabled={saving || downloading} onChange={() => { setValue({ ...value, engine: 'soniox' }); setError(''); }} /><Cloud size={18} /><span><strong>云端</strong><small>Soniox stt-rt-v5</small></span></label>
      </div>
      {cloud ? <>
        <div className="model-health"><Cloud size={20} /><div><strong>Soniox · stt-rt-v5</strong><span>实时传输音频，持续返回识别文字</span></div></div>
        <label className="form-field"><span><KeyRound size={14} /> API Key</span><input type="password" autoComplete="off" spellCheck={false} disabled={adapter.mode === 'demo'} value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder={keyConfigured ? '已配置，填写可替换' : '输入 Soniox API Key'} /></label>
        <p className="settings-help">在 Soniox 控制台创建具有 Speech-to-text, real-time 权限的密钥。密钥安全保存在系统凭据中，重新打开应用后继续可用。</p>
        {keyConfigured && <button className="text-button" onClick={async () => { try { await adapter.dispatch({ type: 'configureSoniox', apiKey: '' }); setKeyConfigured(false); setApiKey(''); } catch { setError('密钥删除失败，请稍后重试。'); } }}>删除已保存密钥</button>}
        <label className="cloud-consent"><input type="checkbox" checked={Boolean(value.cloudConsent)} onChange={(event) => setValue({ ...value, cloudConsent: event.target.checked })} /><span>启用 Soniox 云端识别。选择此引擎后，开始录音、导入音频及重试转写会向 Soniox 发送对应音频；使用费用由我的 Soniox 账户承担。</span></label>
        <p className="settings-help">原始录音仍保存在本机。断网时保留录音，恢复连接后可主动重试转写。</p>
        {adapter.mode === 'demo' && <p className="settings-help">浏览器演示展示设置界面；请在桌面应用中建立连接。</p>}
      </> : <>
        <div className="model-health"><HardDrive size={20} /><div><strong>{downloading ? download?.phase === 'verifying' ? '正在校验模型' : '正在下载模型' : healthTitle}</strong><span>{value.engine === 'whisper' ? 'Whisper 实时录音尚未启用，请选择 Qwen 或 Soniox。' : 'Qwen3-ASR-0.6B（Q8） · 音频在这台电脑上识别'}</span></div></div>
        {value.engine === 'qwen' && (!installed || downloading) && <div className="download-model"><div><strong>下载本地模型</strong><span>约 1.02 GB · 下载后可离线使用</span></div><button className="primary-button" disabled={downloading || runtime.cloudProcessing} onClick={async () => { setInstallState('downloading'); const ok = await onInstall(); setInstallState(ok ? 'done' : 'error'); }}>{downloading ? download?.phase === 'verifying' ? '校验中…' : '下载中…' : download?.phase === 'error' ? '继续下载' : '下载模型'}</button>{downloading && <div className="model-download-progress" role="status"><progress max="100" value={downloadPercent} aria-label="本地模型下载进度" /><span>{download?.phase === 'verifying' ? '正在检查文件完整性…' : `${downloadPercent}% · ${Math.round((download?.downloadedBytes ?? 0) / 1_000_000)} / ${Math.round((download?.totalBytes ?? 1_019_141_728) / 1_000_000)} MB`}</span></div>}{(installState === 'error' || download?.phase === 'error') && <p className="inline-error">{download?.error || '下载暂时中断，已下载的部分会保留。请检查网络后继续。'}</p>}</div>}
        <p className="settings-help">安装包只包含应用程序。本地模型按需下载，已下载的模型可重复使用。</p>
        {value.engine !== 'whisper' && sameEngine && installed && modelState !== 'ready' && <div className="prepare-model"><div>{runtime.modelError && <p className="inline-error">{runtime.modelError}</p>}<span>加载完成后即可开始录音。</span></div><button className="secondary-button" disabled={modelState === 'loading'} onClick={async () => { setPrepareState('loading'); const ok = await onPrepare(); setPrepareState(ok ? 'idle' : 'error'); }}>{modelState === 'loading' ? '正在准备…' : '准备模型'}</button></div>}
        <details className="advanced-settings"><summary>模型文件与引擎路径</summary>{pathField('executable', '引擎程序', runtime.defaultExecutable || '选择识别引擎')}{pathField('modelPath', '模型文件', runtime.defaultModelPath || '选择模型文件')}{value.engine !== 'whisper' && pathField('mmprojPath', '音频投影文件', runtime.defaultMmprojPath || '选择投影文件')}</details>
      </>}
      <label className="form-field"><span>{cloud ? '语言提示' : '识别语言'}</span><select value={value.language} onChange={(event) => setValue({ ...value, language: event.target.value })}><option value="auto">自动检测</option><option value="en">英语</option><option value="zh">中文</option></select></label>
      <label className="form-field vocabulary-field"><span>常用词与标准拼写</span><select aria-label="词表作用范围" value={vocabularyScope} onChange={(event) => setVocabularyScope(event.target.value as typeof vocabularyScope)}><option value="global">所有课程</option>{project && <option value="project">当前课程 · {project.title}</option>}{session && <option value="session">本堂课 · {session.title}</option>}</select><textarea aria-label={`${vocabularyScope === 'session' ? '本堂课' : vocabularyScope === 'project' ? '当前课程' : '所有课程'}常用词与标准拼写`} spellCheck={false} value={visibleVocabulary.join('\n')} maxLength={10000} onChange={(event) => changeVisibleVocabulary(event.target.value.split('\n'))} placeholder={'Amartya Sen\nKahneman\ncon yard → Cournot'} /><small>三层词表会叠加生效，最多使用 200 条；同一误写以本堂课、当前课程、所有课程的顺序优先。每行一个术语，固定纠错可写“常见误写 → 标准拼写”。</small></label>
      <DeepSeekConnection configured={deepseekConfigured} onConfigured={(configured) => { setDeepseekConfigured(configured); if (!configured) setValue((current) => ({ ...current, autoPolish: false })); }} />
      <label className={`auto-polish-control ${deepseekConfigured ? '' : 'is-disabled'}`}>
        <span className="auto-polish-icon"><TextQuote size={17} /></span>
        <span><strong>转写后轻度整理</strong><small>{deepseekConfigured ? '连续片段稳定后按段落整理，并结合下一片段修正跨边界误断句。原始识别稿与撤销历史会保留。' : '先保存 DeepSeek API Key，再启用此功能。'}</small></span>
        <input type="checkbox" role="switch" checked={Boolean(value.autoPolish)} disabled={!deepseekConfigured || saving} onChange={(event) => setValue({ ...value, autoPolish: event.target.checked })} />
      </label>
      {error && <p className="inline-error" role="alert"><CircleAlert size={15} />{error}</p>}
      </div>
      <footer><button className="secondary-button" disabled={saving} onClick={onClose}>取消</button><button className="primary-button" disabled={saving || downloading || value.engine === 'whisper' || (cloud && adapter.mode === 'demo')} onClick={() => void save()}>{saving ? '保存中…' : '保存设置'}</button></footer>
    </section>
  </div>;
}
