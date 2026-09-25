use crate::native_process::SpawnTied;
use crate::{capture, domain::State, store::Store};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
const WHISPER_REALTIME_UNAVAILABLE: &str =
    "Whisper 实时录音尚未启用，请在转写模型中选择 Qwen 或 Soniox。";
const QWEN_MODEL_EXITED: &str = "本地模型进程已退出，请重新准备模型后再开始录音。";
const QWEN_MAX_SEGMENT_SAMPLES: u64 = 8 * 16_000;
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn uid() -> String {
    Uuid::new_v4().to_string()
}
fn id(value: &str) -> Result<&str, String> {
    if !crate::domain::safe_id(value) {
        return Err("无效的课程或录音标识".into());
    }
    Ok(value)
}
fn field(v: &Value, key: &str) -> Result<String, String> {
    v[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("缺少字段 {key}"))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension(format!("{}.part", uid()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        {
            File::open(path.parent().unwrap())?.sync_all()?;
        }
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result.map_err(|e| e.to_string())
}
#[derive(Clone, Serialize, Deserialize, Debug)]
struct Job {
    id: String,
    session: String,
    run: String,
    start: u64,
    end: u64,
    final_result: bool,
    attempts: u32,
    #[serde(default)]
    failed: bool,
}
struct Recording {
    session: String,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}
struct CloudTask {
    session: String,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}
struct AutoPolishLease {
    scheduled: Arc<Mutex<HashSet<String>>>,
    key: String,
}
#[derive(Default)]
struct DeepSeekCredential {
    key: Option<String>,
    epoch: u64,
}
impl AutoPolishLease {
    fn acquire(scheduled: Arc<Mutex<HashSet<String>>>, key: String) -> Option<Self> {
        if !scheduled.lock().ok()?.insert(key.clone()) {
            return None;
        }
        Some(Self { scheduled, key })
    }
}
impl Drop for AutoPolishLease {
    fn drop(&mut self) {
        if let Ok(mut scheduled) = self.scheduled.lock() {
            scheduled.remove(&self.key);
        }
    }
}
fn advance_auto_polish_generation(
    generations: &Arc<Mutex<HashMap<String, u64>>>,
    key: &str,
) -> Option<u64> {
    let mut generations = generations.lock().ok()?;
    let generation = generations.entry(key.to_owned()).or_default();
    *generation = generation.saturating_add(1);
    Some(*generation)
}
fn claim_auto_polish_generation(
    generations: &Arc<Mutex<HashMap<String, u64>>>,
    key: &str,
    generation: u64,
) -> bool {
    let Ok(mut generations) = generations.lock() else {
        return false;
    };
    if generations.get(key).copied() != Some(generation) {
        return false;
    }
    generations.remove(key);
    true
}

fn auto_polish_paragraph_bounds(
    session: &crate::domain::Session,
    trigger: usize,
) -> (usize, usize) {
    let has_open_draft = |id: &str| {
        session
            .drafts
            .iter()
            .any(|draft| draft.segment_id == id && draft.state == "open")
    };
    let merge_safe = |segment: &crate::domain::Segment| {
        segment.final_
            && !segment.display_text.trim().is_empty()
            && !segment.has_human_history()
            && segment.pending_machine.is_none()
            && !has_open_draft(&segment.id)
    };
    if !merge_safe(&session.segments[trigger]) {
        return (trigger, trigger + 1);
    }
    let adjacent = |left: &crate::domain::Segment, right: &crate::domain::Segment| {
        left.run_id == right.run_id
            && right.start_sample >= left.start_sample
            && right.start_sample <= left.end_sample.saturating_add(AUTO_POLISH_MAX_GAP_SAMPLES)
    };
    let mut block_start = trigger;
    let mut block_end = trigger + 1;
    while block_start > 0
        && merge_safe(&session.segments[block_start - 1])
        && adjacent(
            &session.segments[block_start - 1],
            &session.segments[block_start],
        )
    {
        block_start -= 1;
    }
    while block_end < session.segments.len()
        && merge_safe(&session.segments[block_end])
        && adjacent(
            &session.segments[block_end - 1],
            &session.segments[block_end],
        )
    {
        block_end += 1;
    }

    let mut start = block_start;
    while start < block_end {
        let mut end = start;
        let mut characters = 0usize;
        while end < block_end && end - start < 8 {
            let next = session.segments[end].display_text.chars().count();
            if end > start && characters.saturating_add(next) > 12_000 {
                break;
            }
            characters = characters.saturating_add(next);
            end += 1;
        }
        if trigger < end {
            return (start, end);
        }
        let length = end - start;
        start = if length > 2 { end - 2 } else { end };
    }
    (trigger, trigger + 1)
}

/// Speakers often pause mid-sentence for longer than a VAD endpoint; segments
/// separated by up to five seconds may still be one sentence, so they are
/// polished together and the model decides whether to join them.
const AUTO_POLISH_MAX_GAP_SAMPLES: u64 = 80_000;

/// The newest final segment of a run that is still recording (or still has a
/// live hypothesis after it) may be the first half of a sentence. Polishing it
/// alone would add a full stop that later hides the continuation, so it waits
/// until its successor is final or the run closes.
fn auto_polish_tail_open(session: &crate::domain::Session, end: usize) -> bool {
    let Some(last) = end.checked_sub(1).and_then(|index| session.segments.get(index)) else {
        return false;
    };
    let mut later = session.segments[end..]
        .iter()
        .filter(|segment| segment.run_id == last.run_id);
    let mut pending = false;
    for segment in &mut later {
        if segment.final_ {
            return false;
        }
        pending = true;
    }
    pending
        || session
            .runs
            .iter()
            .any(|run| run.id == last.run_id && run.state == "recording")
}

fn auto_polish_run_tail(state: &State, sid: &str, rid: &str) -> Option<String> {
    state
        .sessions
        .iter()
        .find(|session| session.id == sid)?
        .segments
        .iter()
        .rev()
        .find(|segment| {
            segment.run_id == rid && segment.final_ && !segment.display_text.trim().is_empty()
        })
        .map(|segment| segment.id.clone())
}

fn auto_polish_backfill_trigger(state: &State) -> Option<(String, String)> {
    let session = state
        .selected_session_id
        .as_deref()
        .and_then(|id| state.sessions.iter().find(|session| session.id == id))?;
    let segment = session
        .segments
        .iter()
        .rev()
        .find(|segment| segment.final_ && !segment.display_text.trim().is_empty())?;
    if !auto_polish_trigger_safe(session, segment) {
        return None;
    }
    Some((session.id.clone(), segment.id.clone()))
}

fn auto_polish_trigger_safe(
    session: &crate::domain::Session,
    segment: &crate::domain::Segment,
) -> bool {
    let has_open_draft = session
        .drafts
        .iter()
        .any(|draft| draft.segment_id == segment.id && draft.state == "open");
    let has_automatic_history = segment
        .history
        .iter()
        .flatten()
        .any(|correction| correction.automatic);
    let history_is_undone = segment.history_index != segment.history.len() as i64 - 1;
    segment.final_
        && !segment.display_text.trim().is_empty()
        && segment.pending_machine.is_none()
        && !has_open_draft
        && !has_automatic_history
        && !history_is_undone
}
struct AtomicFlagGuard<'a>(&'a AtomicBool);
impl Drop for AtomicFlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    path: PathBuf,
    length: Option<u64>,
    modified: Option<SystemTime>,
}
impl FileFingerprint {
    fn read(path: &Path) -> Self {
        let metadata = path.metadata().ok();
        Self {
            path: path.to_owned(),
            length: metadata.as_ref().map(std::fs::Metadata::len),
            modified: metadata.and_then(|value| value.modified().ok()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalModelVerification {
    model: FileFingerprint,
    mmproj: FileFingerprint,
    verified: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CloudResume {
    cursor: u64,
    ordinal: u64,
}
trait CloudStream: Send {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn finish_input(&mut self) -> Result<(), String>;
    fn poll(&mut self) -> Result<Vec<crate::soniox::Update>, String>;
    fn finished(&self) -> bool;
}
impl CloudStream for crate::soniox::Client {
    fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
        crate::soniox::Client::send_audio(self, bytes)
    }
    fn finish_input(&mut self) -> Result<(), String> {
        crate::soniox::Client::finish_input(self)
    }
    fn poll(&mut self) -> Result<Vec<crate::soniox::Update>, String> {
        crate::soniox::Client::poll(self)
    }
    fn finished(&self) -> bool {
        crate::soniox::Client::finished(self)
    }
}
pub struct Runtime {
    pub store: Arc<Mutex<Store>>,
    root: PathBuf,
    resources: PathBuf,
    capture: Mutex<Option<Recording>>,
    partial: Mutex<BTreeMap<String, Job>>,
    native_receipts: Mutex<()>,
    exit: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    model_child: Arc<Mutex<Option<Child>>>,
    model_status: Mutex<Value>,
    model_download: Mutex<Value>,
    model_installing: AtomicBool,
    local_model_verification: Arc<Mutex<Vec<LocalModelVerification>>>,
    prepare_requested: AtomicBool,
    persist_credentials: bool,
    soniox_key: Mutex<Option<String>>,
    deepseek_credential: Arc<Mutex<DeepSeekCredential>>,
    auto_polish_scheduled: Arc<Mutex<HashSet<String>>>,
    /// Outcome of the most recent automatic polish, so a failure is visible.
    auto_polish_status: Arc<Mutex<Value>>,
    auto_polish_generation: Arc<Mutex<HashMap<String, u64>>>,
    cloud: Mutex<Option<CloudTask>>,
}
impl Runtime {
    pub fn new(root: PathBuf, resources: PathBuf) -> Result<Arc<Self>, String> {
        fs::create_dir_all(root.join("audio")).map_err(|e| e.to_string())?;
        fs::create_dir_all(root.join("jobs")).map_err(|e| e.to_string())?;
        let mut store = Store::open_or_recover(&root.join("lectureedit.sqlite"))?;
        let defaults = crate::models::defaults(&root, &resources);
        store.mutate(|s| {
            let mut v = serde_json::to_value(&*s).map_err(|e| e.to_string())?;
            v["workerEpoch"] = json!(v["workerEpoch"].as_u64().unwrap_or(0) + 1);
            if v["settings"]["executable"]
                .as_str()
                .unwrap_or("")
                .is_empty()
            {
                let engine = v["settings"]["engine"].clone();
                let language = v["settings"]["language"].clone();
                let consent = v["settings"]["cloudConsent"].clone();
                let vocabulary = v["settings"]["customVocabulary"].clone();
                let auto_polish = v["settings"]["autoPolish"].clone();
                let theme = v["settings"]["theme"].clone();
                v["settings"] = defaults;
                v["settings"]["theme"] = if theme.is_string() {
                    theme
                } else {
                    json!("system")
                };
                if vocabulary.is_array() {
                    v["settings"]["customVocabulary"] = vocabulary;
                }
                if auto_polish.is_boolean() {
                    v["settings"]["autoPolish"] = auto_polish;
                }
                if engine == "soniox" {
                    v["settings"]["engine"] = engine;
                    v["settings"]["language"] = language;
                    v["settings"]["cloudConsent"] = consent;
                }
            } else if v["settings"]["engine"] == "qwen"
                && v["settings"]["executable"].as_str().is_some_and(|p| {
                    let normalised = p.replace('\\', "/");
                    normalised.contains("/Contents/Resources/native/qwen/")
                        || normalised.contains("/src-tauri/resources/native/qwen/")
                        || (cfg!(windows) && normalised.ends_with("/native/qwen/llama-server.exe"))
                })
            {
                v["settings"]["executable"] = defaults["executable"].clone();
            }
            *s = serde_json::from_value(v).map_err(|e| e.to_string())?;
            Ok(())
        })?;
        let soniox_key = crate::credentials::load(crate::credentials::CredentialKind::Soniox)
            .ok()
            .flatten();
        let deepseek_key = crate::credentials::load(crate::credentials::CredentialKind::DeepSeek)
            .ok()
            .flatten();
        let runtime = Arc::new(Self {
            store: Arc::new(Mutex::new(store)),
            root,
            resources,
            capture: Mutex::new(None),
            partial: Mutex::new(BTreeMap::new()),
            native_receipts: Mutex::new(()),
            exit: AtomicBool::new(false),
            worker: Mutex::new(None),
            model_child: Arc::new(Mutex::new(None)),
            model_status: Mutex::new(json!({"state":"unloaded"})),
            model_download: Mutex::new(json!({
                "phase":"idle",
                "downloadedBytes":0,
                "totalBytes":crate::models::install_total_bytes(),
                "fileName":Value::Null,
                "error":Value::Null
            })),
            model_installing: AtomicBool::new(false),
            local_model_verification: Arc::new(Mutex::new(Vec::new())),
            prepare_requested: AtomicBool::new(true),
            persist_credentials: true,
            soniox_key: Mutex::new(soniox_key),
            deepseek_credential: Arc::new(Mutex::new(DeepSeekCredential {
                key: deepseek_key,
                epoch: 0,
            })),
            auto_polish_scheduled: Arc::new(Mutex::new(HashSet::new())),
            auto_polish_status: Arc::new(Mutex::new(json!({"state":"idle"}))),
            auto_polish_generation: Arc::new(Mutex::new(HashMap::new())),
            cloud: Mutex::new(None),
        });
        runtime.recover()?;
        let weak = Arc::downgrade(&runtime);
        let worker = thread::spawn(move || worker_loop(weak));
        *runtime.worker.lock().unwrap() = Some(worker);
        Ok(runtime)
    }
    fn snapshot(&self) -> Result<State, String> {
        Ok(self
            .store
            .lock()
            .map_err(|_| "数据写入器不可用")?
            .snapshot())
    }
    fn value(&self) -> Result<Value, String> {
        serde_json::to_value(self.snapshot()?).map_err(|e| e.to_string())
    }
    fn session(&self, sid: &str) -> Result<Value, String> {
        let store = self.store.lock().map_err(|_| "数据写入器不可用")?;
        let session = store
            .state()
            .sessions
            .iter()
            .find(|session| session.id == sid)
            .ok_or("课程不存在")?;
        serde_json::to_value(session).map_err(|e| e.to_string())
    }
    fn update<F>(&self, sid: &str, f: F) -> Result<(), String>
    where
        F: FnOnce(&mut Value) -> Result<(), String>,
    {
        self.store
            .lock()
            .map_err(|_| "数据写入器不可用")?
            .change(|s| {
                // Round-trip only this lecture through JSON, not the library.
                let slot = s
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == sid)
                    .ok_or("课程不存在")?;
                let mut session = serde_json::to_value(&*slot).map_err(|e| e.to_string())?;
                f(&mut session)?;
                session["operationSeq"] = json!(session["operationSeq"].as_u64().unwrap_or(0) + 1);
                *slot = serde_json::from_value(session).map_err(|e| e.to_string())?;
                Ok(())
            })
    }
    fn error(&self, sid: &str, msg: &str, inference: bool) {
        let _ = self.update(sid, |s| {
            s["error"] = json!(msg);
            s[if inference {
                "inferenceState"
            } else {
                "recordingState"
            }] = json!("error");
            Ok(())
        });
    }
    pub fn sync(
        self: &Arc<Self>,
        command: Option<Value>,
        epoch: &str,
        since: u64,
    ) -> Result<Box<serde_json::value::RawValue>, String> {
        if let Some(command) = command.filter(|command| command["type"] != "snapshot") {
            self.dispatch(command)?;
        }
        self.store
            .lock()
            .map_err(|_| "数据写入器不可用")?
            .delta_json(epoch, since)
    }
    pub fn dispatch(self: &Arc<Self>, command: Value) -> Result<State, String> {
        let kind = command["type"].as_str().unwrap_or("");
        if kind == "configureSoniox" {
            let key = field(&command, "apiKey")?;
            if key.len() > 256 {
                return Err("Soniox API 密钥格式无效".into());
            }
            let configured = !key.is_empty();
            if self.persist_credentials {
                crate::credentials::save(
                    crate::credentials::CredentialKind::Soniox,
                    configured.then_some(key.as_str()),
                )?;
            }
            *self.soniox_key.lock().map_err(|_| "Soniox 密钥不可用")? = configured.then_some(key);
            if !configured {
                if let Some(task) = self
                    .cloud
                    .lock()
                    .map_err(|_| "云端转写控制器不可用")?
                    .as_ref()
                {
                    task.stop.store(true, Ordering::SeqCst);
                }
            }
            return self.snapshot();
        }
        if kind == "configureDeepSeek" {
            let key = field(&command, "apiKey")?;
            if key.len() > 512 || (!key.is_empty() && key.chars().any(char::is_whitespace)) {
                return Err("DeepSeek API 密钥格式无效".into());
            }
            let configured = !key.is_empty();
            if self.persist_credentials {
                crate::credentials::save(
                    crate::credentials::CredentialKind::DeepSeek,
                    configured.then_some(key.as_str()),
                )?;
            }
            {
                let mut credential = self
                    .deepseek_credential
                    .lock()
                    .map_err(|_| "DeepSeek 密钥不可用")?;
                credential.key = configured.then_some(key);
                credential.epoch = credential.epoch.saturating_add(1);
            }
            if !configured {
                let mut settings = self.snapshot()?.settings;
                if settings.auto_polish {
                    settings.auto_polish = false;
                    self.store
                        .lock()
                        .map_err(|_| "数据写入器不可用")?
                        .dispatch(json!({
                                "type":"settings",
                                "commandId":uid(),
                                "settings":settings
                        }))?;
                }
            } else if let Some((session_id, segment_id)) =
                auto_polish_backfill_trigger(&self.snapshot()?)
            {
                self.schedule_auto_polish(&session_id, &segment_id);
            }
            return self.snapshot();
        }
        if kind == "cancelCloud" {
            let sid = field(&command, "sessionId")?;
            self.reap_cloud();
            let cloud = self.cloud.lock().map_err(|_| "云端转写控制器不可用")?;
            let task = cloud.as_ref().ok_or("当前没有云端转写任务")?;
            if task.session != sid {
                return Err("云端转写属于另一门课程".into());
            }
            task.stop.store(true, Ordering::SeqCst);
            drop(cloud);
            self.update(&sid, |session| {
                session["inferenceState"] = json!("paused");
                session["error"] = Value::Null;
                Ok(())
            })?;
            return self.snapshot();
        }
        if ![
            "startRecording",
            "pauseRecording",
            "stopRecording",
            "importAudio",
            "importPackage",
            "export",
            "installModel",
        ]
        .contains(&kind)
        {
            return self.dispatch_inner(command);
        }
        let command_id = field(&command, "commandId")?;
        if !crate::domain::safe_id(&command_id) {
            return Err("无效的操作标识".into());
        }
        let receipt = self
            .root
            .join("native-operations")
            .join(format!("{command_id}.json"));
        {
            let _guard = self.native_receipts.lock().map_err(|_| "操作记录不可用")?;
            if receipt.exists() {
                let previous: Value =
                    serde_json::from_slice(&fs::read(&receipt).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                if previous["command"] != command {
                    return Err("COMMAND_REUSED_WITH_DIFFERENT_PAYLOAD".into());
                }
                return match previous["status"].as_str() {
                    Some("done") => self.snapshot(),
                    Some("failed") => Err(previous["error"]
                        .as_str()
                        .unwrap_or("上次操作失败")
                        .to_owned()),
                    _ => Err("此操作仍在执行或上次执行已中断；已保存的数据保留在课程中".into()),
                };
            }
            atomic_write(
                &receipt,
                &serde_json::to_vec(&json!({"command":command,"status":"started"})).unwrap(),
            )?;
        }
        let result = self.dispatch_inner(command.clone());
        let record = match &result {
            Ok(_) => json!({"command":command,"status":"done"}),
            Err(e) => json!({"command":command,"status":"failed","error":e}),
        };
        atomic_write(&receipt, &serde_json::to_vec(&record).unwrap())?;
        result
    }
    fn dispatch_inner(self: &Arc<Self>, command: Value) -> Result<State, String> {
        match command["type"].as_str().unwrap_or("") {
            "prepareModel" => {
                self.prepare_requested.store(true, Ordering::SeqCst);
                self.snapshot()
            }
            "startRecording" => {
                self.start(
                    &field(&command, "sessionId")?,
                    command["source"].as_str().unwrap_or("microphone"),
                )?;
                self.snapshot()
            }
            "pauseRecording" => {
                self.pause(&field(&command, "sessionId")?)?;
                self.snapshot()
            }
            "stopRecording" => {
                self.stop(&field(&command, "sessionId")?)?;
                self.snapshot()
            }
            "importAudio" => {
                self.import_audio(
                    &field(&command, "sessionId")?,
                    Path::new(&field(&command, "path")?),
                )?;
                self.snapshot()
            }
            "retryInference" => {
                let sid = field(&command, "sessionId")?;
                let cloud_selected = self.value()?["settings"]["engine"] == "soniox";
                if cloud_selected {
                    self.soniox_configuration(&sid)?;
                } else {
                    self.prepare_requested.store(true, Ordering::SeqCst);
                }
                let prepare = || -> Result<(), String> {
                    self.store.lock().unwrap().mutate(|s| {
                        s.worker_epoch += 1;
                        Ok(())
                    })?;
                    for mut job in self.jobs()? {
                        if job.session == sid && self.job_is_cloud(&job)? == cloud_selected {
                            job.failed = false;
                            job.attempts = 0;
                            self.save_job(&job)?;
                        }
                    }
                    self.update(&sid, |s| {
                        s["error"] = Value::Null;
                        s["inferenceState"] = json!("catching_up");
                        Ok(())
                    })?;
                    Ok(())
                };
                if cloud_selected {
                    self.spawn_cloud_retry_with(sid.clone(), prepare)?;
                } else {
                    prepare()?;
                }
                self.snapshot()
            }
            "export" => {
                self.export(&command)?;
                self.snapshot()
            }
            "importPackage" => {
                let session = crate::archive::import_lecture(
                    Path::new(&field(&command, "path")?),
                    &self.root.join("audio"),
                )?;
                let audio = self.root.join("audio").join(id(&session.id)?);
                let imported = self
                    .store
                    .lock()
                    .map_err(|_| "数据写入器不可用".to_owned())
                    .and_then(|mut store| store.dispatch(json!({"type":"importSession","commandId":command["commandId"],"session":session})));
                if imported.is_err() {
                    let _ = fs::remove_dir_all(audio);
                }
                imported
            }
            "installModel" => {
                if self
                    .model_installing
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return Err("模型正在下载或校验，请稍后再试".into());
                }
                let _installing = AtomicFlagGuard(&self.model_installing);
                if self
                    .capture
                    .lock()
                    .map_err(|_| "录音控制器不可用")?
                    .is_some()
                {
                    return Err("请结束录音后安装模型".into());
                }
                self.reap_cloud();
                if self
                    .cloud
                    .lock()
                    .map_err(|_| "云端转写控制器不可用")?
                    .is_some()
                {
                    return Err("Soniox 转写正在进行，请完成或暂停后再安装本地模型".into());
                }
                *self
                    .model_download
                    .lock()
                    .map_err(|_| "模型下载状态不可用")? = json!({
                    "phase":"verifying",
                    "downloadedBytes":0,
                    "totalBytes":crate::models::install_total_bytes(),
                    "fileName":Value::Null,
                    "error":Value::Null
                });
                let mut last_publish = Instant::now()
                    .checked_sub(Duration::from_millis(200))
                    .unwrap_or_else(Instant::now);
                let mut last_phase = String::new();
                let mut latest_progress = self
                    .model_download
                    .lock()
                    .map_err(|_| "模型下载状态不可用")?
                    .clone();
                let install = crate::models::install(&self.root, &self.resources, |progress| {
                    let phase_changed = progress.phase != last_phase;
                    let value = serde_json::to_value(&progress).unwrap_or_else(|_| {
                        json!({
                            "phase":progress.phase,
                            "downloadedBytes":progress.downloaded_bytes,
                            "totalBytes":progress.total_bytes,
                            "fileName":progress.file_name.clone(),
                            "error":progress.error.clone()
                        })
                    });
                    latest_progress = value.clone();
                    if phase_changed
                        || progress.phase == "complete"
                        || last_publish.elapsed() >= Duration::from_millis(200)
                    {
                        if let Ok(mut current) = self.model_download.lock() {
                            *current = value;
                        }
                        last_phase = progress.phase.to_owned();
                        last_publish = Instant::now();
                    }
                });
                let installed = match install {
                    Ok(settings) => settings,
                    Err(error) => {
                        if let Ok(mut progress) = self.model_download.lock() {
                            *progress = latest_progress;
                            progress["phase"] = json!("error");
                            progress["error"] = json!(error);
                        }
                        return Err(error);
                    }
                };
                let mut store = self.store.lock().map_err(|_| "数据写入器不可用")?;
                let mut settings = serde_json::to_value(store.snapshot().settings)
                    .map_err(|error| error.to_string())?;
                settings["engine"] = json!("qwen");
                settings["executable"] = installed["executable"].clone();
                settings["modelPath"] = installed["modelPath"].clone();
                settings["mmprojPath"] = installed["mmprojPath"].clone();
                self.remember_verified_local_qwen(&settings);
                let state = store
                    .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))?;
                self.prepare_requested.store(true, Ordering::SeqCst);
                Ok(state)
            }
            "settings" => {
                if self.model_installing.load(Ordering::SeqCst) {
                    return Err("模型正在下载或校验，请完成后再更改设置".into());
                }
                if self.capture.lock().unwrap().is_some() {
                    let current = self.value()?["settings"].clone();
                    let incoming = command.get("settings").cloned().ok_or("MISSING_SETTINGS")?;
                    let transcription_keys = [
                        "engine",
                        "executable",
                        "modelPath",
                        "mmprojPath",
                        "language",
                        "customVocabulary",
                        "cloudConsent",
                    ];
                    if transcription_keys
                        .iter()
                        .any(|key| current[*key] != incoming[*key])
                    {
                        return Err("请先结束录音，再切换转写设置".into());
                    }
                }
                let incoming = command.get("settings").cloned().ok_or("MISSING_SETTINGS")?;
                // The local worker idles while Soniox is selected, so queued Qwen audio would strand.
                if incoming["engine"] == "soniox" && self.value()?["settings"]["engine"] != "soniox"
                {
                    let pending = self
                        .jobs()?
                        .iter()
                        .filter(|job| !job.failed && !self.job_is_cloud(job).unwrap_or(false))
                        .count();
                    if pending > 0 {
                        return Err(format!(
                            "还有 {pending} 段音频正在本地转写，请等待完成后再切换到 Soniox"
                        ));
                    }
                }
                if incoming["autoPolish"] == true
                    && self
                        .deepseek_credential
                        .lock()
                        .map_err(|_| "DeepSeek 密钥不可用")?
                        .key
                        .is_none()
                {
                    return Err("请先配置 DeepSeek API Key，再启用转写轻度整理".into());
                }
                let mut store = self.store.lock().unwrap();
                if self.model_installing.load(Ordering::SeqCst) {
                    return Err("模型正在下载或校验，请完成后再更改设置".into());
                }
                let auto_polish_was_enabled = store.snapshot().settings.auto_polish;
                let state = store.dispatch(command)?;
                drop(store);
                if auto_polish_was_enabled != state.settings.auto_polish {
                    let mut credential = self
                        .deepseek_credential
                        .lock()
                        .map_err(|_| "DeepSeek 密钥不可用")?;
                    credential.epoch = credential.epoch.saturating_add(1);
                }
                if !auto_polish_was_enabled && state.settings.auto_polish {
                    if let Some((session_id, segment_id)) = auto_polish_backfill_trigger(&state) {
                        self.schedule_auto_polish(&session_id, &segment_id);
                    }
                }
                Ok(state)
            }
            _ => self
                .store
                .lock()
                .map_err(|_| "数据写入器不可用")?
                .dispatch(command),
        }
    }
    fn run_path(&self, sid: &str, rid: &str) -> Result<PathBuf, String> {
        Ok(self
            .root
            .join("audio")
            .join(id(sid)?)
            .join(format!("{}.pcm", id(rid)?)))
    }
    fn helper(&self) -> PathBuf {
        let name = if cfg!(target_os = "windows") {
            "lectureedit-loopback.exe"
        } else {
            "lectureedit-capture"
        };
        let candidates = [
            self.resources.join("native").join(name),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../native/bin")
                .join(name),
        ];
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .unwrap_or_default()
    }
    fn local_qwen_installed(&self, settings: &Value) -> bool {
        if !Path::new(settings["executable"].as_str().unwrap_or("")).is_file() {
            return false;
        }
        let model = PathBuf::from(settings["modelPath"].as_str().unwrap_or(""));
        let mmproj = PathBuf::from(settings["mmprojPath"].as_str().unwrap_or(""));
        if !model.is_file() || !mmproj.is_file() {
            return false;
        }
        if !crate::models::uses_download_manifest(&model, &mmproj) {
            return true;
        }
        if !crate::models::downloaded_file_sizes_match(&model, &mmproj) {
            return false;
        }
        let candidate = LocalModelVerification {
            model: FileFingerprint::read(&model),
            mmproj: FileFingerprint::read(&mmproj),
            verified: None,
        };
        if let Ok(mut cache) = self.local_model_verification.lock() {
            if let Some(cached) = cache
                .iter()
                .find(|cached| cached.model == candidate.model && cached.mmproj == candidate.mmproj)
            {
                return cached.verified.unwrap_or(false);
            }
            cache.retain(|entry| {
                entry.model.path != candidate.model.path
                    || entry.mmproj.path != candidate.mmproj.path
            });
            cache.push(candidate.clone());
        }
        let cache = self.local_model_verification.clone();
        thread::spawn(move || {
            let verified = crate::models::downloaded_files_verified(&model, &mmproj);
            if let Ok(mut cache) = cache.lock() {
                if let Some(entry) = cache.iter_mut().find(|entry| {
                    entry.model == candidate.model && entry.mmproj == candidate.mmproj
                }) {
                    entry.verified = Some(verified);
                }
            }
        });
        false
    }
    fn remember_verified_local_qwen(&self, settings: &Value) {
        let model = PathBuf::from(settings["modelPath"].as_str().unwrap_or(""));
        let mmproj = PathBuf::from(settings["mmprojPath"].as_str().unwrap_or(""));
        if crate::models::uses_download_manifest(&model, &mmproj) {
            if let Ok(mut cache) = self.local_model_verification.lock() {
                let verified = LocalModelVerification {
                    model: FileFingerprint::read(&model),
                    mmproj: FileFingerprint::read(&mmproj),
                    verified: Some(true),
                };
                cache.retain(|entry| {
                    entry.model.path != verified.model.path
                        || entry.mmproj.path != verified.mmproj.path
                });
                cache.push(verified);
            }
        }
    }
    pub fn info(&self) -> Value {
        let mut info = self.runtime_status();
        info["autoPolish"] = self
            .auto_polish_status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| json!({"state":"idle"}));
        info["recovered"] = self
            .store
            .lock()
            .map(|store| json!(store.recovered()))
            .unwrap_or_else(|_| json!([]));
        info
    }
    fn runtime_status(&self) -> Value {
        let settings = self
            .store
            .lock()
            .ok()
            .and_then(|store| serde_json::to_value(&store.state().settings).ok())
            .unwrap_or_default();
        let defaults = crate::models::defaults(&self.root, &self.resources);
        let local_model_installed = match settings["engine"].as_str() {
            Some("qwen" | "qwen3-asr") => self.local_qwen_installed(&settings),
            Some("soniox") => {
                self.local_qwen_installed(&settings) || self.local_qwen_installed(&defaults)
            }
            _ => self.local_qwen_installed(&defaults),
        };
        let model_download = self
            .model_download
            .lock()
            .map(|progress| progress.clone())
            .unwrap_or_else(|_| {
                json!({
                    "phase":"error",
                    "downloadedBytes":0,
                    "totalBytes":crate::models::install_total_bytes(),
                    "fileName":Value::Null,
                    "error":"模型下载状态不可用"
                })
            });
        let (cloud_processing, cloud_session_id) = self
            .cloud
            .lock()
            .map(|cloud| {
                cloud
                    .as_ref()
                    .filter(|task| !task.thread.is_finished())
                    .map(|task| (true, Value::String(task.session.clone())))
                    .unwrap_or((false, Value::Null))
            })
            .unwrap_or((false, Value::Null));
        let cloud_key_configured = self
            .soniox_key
            .lock()
            .is_ok_and(|key| key.as_ref().is_some_and(|key| !key.is_empty()));
        let deepseek_key_configured = self
            .deepseek_credential
            .lock()
            .is_ok_and(|credential| credential.key.as_ref().is_some_and(|key| !key.is_empty()));
        if settings["engine"] == "whisper" {
            return json!({"platform":std::env::consts::OS,"modelInstalled":model_files_present(&settings),"localModelInstalled":local_model_installed,"modelDownload":model_download,"modelReady":false,"modelState":"error","modelError":WHISPER_REALTIME_UNAVAILABLE,"cloudKeyConfigured":cloud_key_configured,"deepseekKeyConfigured":deepseek_key_configured,"cloudProcessing":cloud_processing,"cloudSessionId":cloud_session_id,"defaultExecutable":defaults["executable"],"defaultModelPath":defaults["modelPath"],"defaultMmprojPath":defaults["mmprojPath"],"dataDirectory":self.root,"systemAudioAvailable":self.helper().is_file(),"defaults":defaults});
        }
        if settings["engine"] == "soniox" {
            let ready = settings["cloudConsent"] == true && cloud_key_configured;
            return json!({"platform":std::env::consts::OS,"modelInstalled":true,"localModelInstalled":local_model_installed,"modelDownload":model_download,"modelReady":ready,"modelState":if ready {"ready"} else {"unloaded"},"modelError":Value::Null,"cloudKeyConfigured":cloud_key_configured,"deepseekKeyConfigured":deepseek_key_configured,"cloudProcessing":cloud_processing,"cloudSessionId":cloud_session_id,"defaultExecutable":defaults["executable"],"defaultModelPath":defaults["modelPath"],"defaultMmprojPath":defaults["mmprojPath"],"dataDirectory":self.root,"systemAudioAvailable":self.helper().is_file(),"defaults":defaults});
        }
        let installed = model_files_present(&settings);
        let status = self.checked_model_status(&settings);
        let current = same_model_settings(&status["settings"], &settings);
        let ready = current && status["state"] == "ready";
        json!({"platform":std::env::consts::OS,"modelInstalled":installed,"localModelInstalled":local_model_installed,"modelDownload":model_download,"modelReady":ready,"modelState":if current {status["state"].clone()} else {json!(if installed {"loading"} else {"unloaded"})},"modelError":if current {status["error"].clone()} else {Value::Null},"cloudKeyConfigured":cloud_key_configured,"deepseekKeyConfigured":deepseek_key_configured,"cloudProcessing":cloud_processing,"cloudSessionId":cloud_session_id,"defaultExecutable":defaults["executable"],"defaultModelPath":defaults["modelPath"],"defaultMmprojPath":defaults["mmprojPath"],"dataDirectory":self.root,"systemAudioAvailable":self.helper().is_file(),"defaults":defaults})
    }
    fn checked_model_status(&self, settings: &Value) -> Value {
        let mut status = self.model_status.lock().unwrap();
        if matches!(settings["engine"].as_str(), Some("qwen" | "qwen3-asr"))
            && same_model_settings(&status["settings"], settings)
            && status["state"] == "ready"
        {
            let mut child = self.model_child.lock().unwrap();
            let failure = match child.as_mut() {
                Some(process) => match process.try_wait() {
                    Ok(None) => false,
                    Ok(Some(_)) => {
                        child.take();
                        true
                    }
                    Err(_) => true,
                },
                None => true,
            };
            if failure {
                *status = json!({
                    "state":"error",
                    "settings":settings,
                    "error":QWEN_MODEL_EXITED
                });
            }
        }
        status.clone()
    }
    fn deepseek_api_key(&self) -> Result<String, String> {
        self.deepseek_credential
            .lock()
            .map_err(|_| "DeepSeek 密钥不可用")?
            .key
            .clone()
            .filter(|key| !key.is_empty())
            .ok_or("请先配置 DeepSeek API 密钥".into())
    }
    pub fn translate_text(
        &self,
        request: crate::deepseek::TranslateRequest,
    ) -> Result<crate::deepseek::TranslateResult, String> {
        let key = self.deepseek_api_key()?;
        crate::deepseek::Client::new()?.translate(&key, request)
    }
    pub fn recognize_formula(
        &self,
        request: crate::deepseek::FormulaRequest,
    ) -> Result<crate::deepseek::FormulaResult, String> {
        let key = self.deepseek_api_key()?;
        crate::deepseek::Client::new()?.recognize_formula(&key, request)
    }

    fn schedule_auto_polish(&self, sid: &str, segment_id: &str) {
        let Ok(store) = self.store.lock() else { return };
        let state = store.state();
        if !state.settings.auto_polish {
            return;
        }
        let (key, epoch) = {
            let Ok(credential) = self.deepseek_credential.lock() else {
                return;
            };
            let Some(key) = credential.key.clone() else {
                return;
            };
            (key, credential.epoch)
        };
        let Some((session, segment)) = state
            .sessions
            .iter()
            .find(|session| session.id == sid)
            .and_then(|session| {
                session
                    .segments
                    .iter()
                    .find(|segment| segment.id == segment_id)
                    .map(|segment| (session, segment))
            })
        else {
            return;
        };
        if !auto_polish_trigger_safe(session, segment) {
            return;
        }
        let generation_key = format!("{sid}\0{}", segment.run_id);
        drop(store);
        let store = self.store.clone();
        let status = self.auto_polish_status.clone();
        let session_id = sid.to_owned();
        let trigger_id = segment_id.to_owned();
        let generations = self.auto_polish_generation.clone();
        let Some(generation) = advance_auto_polish_generation(&generations, &generation_key) else {
            return;
        };
        let scheduled = self.auto_polish_scheduled.clone();
        let credential = self.deepseek_credential.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(2_200));
            if !claim_auto_polish_generation(&generations, &generation_key, generation) {
                return;
            }
            let Ok(guard) = store.lock() else { return };
            let snapshot = guard.state();
            if !snapshot.settings.auto_polish {
                return;
            }
            if credential.lock().map_or(true, |value| value.epoch != epoch) {
                return;
            }
            let Some(session) = snapshot
                .sessions
                .iter()
                .find(|session| session.id == session_id)
            else {
                return;
            };
            let Some(trigger) = session
                .segments
                .iter()
                .position(|segment| segment.id == trigger_id)
            else {
                return;
            };
            let (start, end) = auto_polish_paragraph_bounds(session, trigger);
            let tail_open = auto_polish_tail_open(session, end);
            if tail_open && end - start < 2 {
                return;
            }
            // Copy just this paragraph so the library is unlocked during the request.
            let candidates = session.segments[start..end].to_vec();
            drop(guard);
            let schedule_key = candidates
                .iter()
                .map(|segment| {
                    format!(
                        "{}:{}:{}",
                        segment.id, segment.machine_revision, segment.user_seq
                    )
                })
                .collect::<Vec<_>>()
                .join("|");
            let Some(_lease) = AutoPolishLease::acquire(
                scheduled,
                format!("{session_id}\0{epoch}\0{schedule_key}"),
            ) else {
                return;
            };
            let texts: Vec<String> = candidates
                .iter()
                .map(|segment| segment.display_text.clone())
                .collect();
            let result = crate::deepseek::Client::new().and_then(|client| {
                client.polish_segments(
                    &key,
                    crate::deepseek::PolishSegmentsRequest {
                        segments: texts,
                        consent: true,
                    },
                )
            });
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    if let Ok(mut status) = status.lock() {
                        *status = json!({"state":"error","message":error,"sessionId":session_id,"at":now()});
                    }
                    return;
                }
            };
            if credential.lock().map_or(true, |value| value.epoch != epoch) {
                return;
            }
            let mut outputs = result.segments;
            let mut source_count = candidates.len();
            if tail_open {
                // Keep the group that contains the unfinished tail untouched;
                // it is polished together with its continuation later.
                source_count = outputs.pop().map_or(0, |group| group.source_start);
                if source_count == 0 {
                    return;
                }
            }
            let sources: Vec<Value> = candidates[..source_count]
                .iter()
                .map(|segment| {
                    json!({
                        "id":segment.id,
                        "expectedMachineRevision":segment.machine_revision,
                        "expectedUserSeq":segment.user_seq,
                        "sourceText":segment.display_text
                    })
                })
                .collect();
            let Ok(mut store) = store.lock() else { return };
            if !store.state().settings.auto_polish {
                return;
            }
            let Ok(credential_guard) = credential.lock() else {
                return;
            };
            if credential_guard.epoch != epoch {
                return;
            }
            let applied = store.apply(json!({
                "type":"polishSegments",
                "commandId":uid(),
                "sessionId":session_id,
                "sources":sources,
                "segments":outputs
            }));
            drop(credential_guard);
            drop(store);
            if let Ok(mut status) = status.lock() {
                // A conflict means the text changed meanwhile; that is not a failure.
                if applied.is_ok() || status["state"] == "error" {
                    *status = json!({"state":"ok","at":now()});
                }
            }
        });
    }
    fn soniox_configuration(&self, sid: &str) -> Result<(String, String, Vec<String>), String> {
        let state = self.value()?;
        let settings = effective_session_settings(&state, sid);
        if settings["cloudConsent"] != true {
            return Err("请先同意将音频发送到 Soniox 云端".into());
        }
        let key = self
            .soniox_key
            .lock()
            .map_err(|_| "Soniox 密钥不可用")?
            .clone()
            .filter(|key| !key.is_empty())
            .ok_or("请先输入 Soniox API 密钥")?;
        let language = match settings["language"].as_str().unwrap_or("auto") {
            "en" => "en".to_owned(),
            "zh" => "zh".to_owned(),
            "auto" | "" => "auto".to_owned(),
            _ => return Err("Soniox 识别语言设置无效".into()),
        };
        Ok((key, language, vocabulary_terms(&settings)))
    }
    fn add_run(&self, sid: &str, source: &str) -> Result<String, String> {
        id(sid)?;
        let rid = uid();
        let engine = self.value()?["settings"]["engine"]
            .as_str()
            .unwrap_or("qwen")
            .to_owned();
        self.update(sid,|s|{if s["mode"]=="demo"{return Err("请新建课程后录音或导入音频".into());}let runs=s["runs"].as_array_mut().ok_or("无效的录音列表")?;let offset:u64=runs.iter().map(|r|r["samples"].as_u64().unwrap_or(0)*1000/16000).sum();runs.push(json!({"id":rid,"source":source,"engine":engine,"startedAt":now(),"endedAt":null,"samples":0,"offsetMs":offset,"state":"recording"}));s["error"]=Value::Null;s["recordingState"]=json!("starting");Ok(())})?;
        Ok(rid)
    }
    fn run_engine(&self, sid: &str, rid: &str) -> Result<Option<String>, String> {
        Ok(self.session(sid)?["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|run| run["id"] == rid))
            .and_then(|run| run["engine"].as_str())
            .map(str::to_owned)
            .or_else(|| Some("qwen".into())))
    }
    fn job_is_cloud(&self, job: &Job) -> Result<bool, String> {
        Ok(self.run_engine(&job.session, &job.run)?.as_deref() == Some("soniox"))
    }
    fn cloud_upload_allowed(&self, epoch: u64) -> Result<(), String> {
        let state = self.value()?;
        if state["workerEpoch"].as_u64().unwrap_or(0) != epoch {
            return Err("STALE_WORKER_EPOCH".into());
        }
        if state["settings"]["engine"] != "soniox"
            || state["settings"]["cloudConsent"] != true
            || self
                .soniox_key
                .lock()
                .map_err(|_| "Soniox 密钥不可用")?
                .as_ref()
                .is_none_or(String::is_empty)
        {
            return Err("Soniox 云端转写授权已关闭".into());
        }
        Ok(())
    }
    fn reap_cloud(&self) {
        let completed = self.cloud.lock().ok().and_then(|mut cloud| {
            cloud
                .as_ref()
                .is_some_and(|task| task.thread.is_finished())
                .then(|| cloud.take())
                .flatten()
        });
        if let Some(task) = completed {
            let _ = task.thread.join();
        }
    }
    fn spawn_cloud_live(
        self: &Arc<Self>,
        sid: String,
        rid: String,
        client: crate::soniox::Client,
    ) -> Result<(), String> {
        let mut slot = self.cloud.lock().map_err(|_| "云端转写控制器不可用")?;
        if self.model_installing.load(Ordering::SeqCst) {
            return Err("本地模型正在安装，请完成后再开始 Soniox 转写".into());
        }
        if slot.is_some() {
            return Err("另一项 Soniox 转写正在进行，请稍后再试".into());
        }
        let epoch = self.snapshot()?.worker_epoch;
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let weak = Arc::downgrade(self);
        let task_session = sid.clone();
        let thread = thread::spawn(move || {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            let resume = runtime.cloud_resume(&sid, &rid).unwrap_or_default();
            let result = cloud_run(&runtime, &sid, &rid, client, resume, epoch, &cancel);
            runtime.finish_cloud_attempt(&sid, &rid, result);
        });
        *slot = Some(CloudTask {
            session: task_session,
            stop,
            thread,
        });
        Ok(())
    }
    fn spawn_cloud_retry_with<F>(self: &Arc<Self>, sid: String, prepare: F) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        self.reap_cloud();
        let mut slot = self.cloud.lock().map_err(|_| "云端转写控制器不可用")?;
        if self.model_installing.load(Ordering::SeqCst) {
            return Err("本地模型正在安装，请完成后再开始 Soniox 转写".into());
        }
        if slot.is_some() {
            return Err("另一项 Soniox 转写正在进行，请稍后再试".into());
        }
        prepare()?;
        let epoch = self.snapshot()?.worker_epoch;
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let weak = Arc::downgrade(self);
        let task_session = sid.clone();
        let thread = thread::spawn(move || {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            let result = runtime.run_cloud_retries(&sid, epoch, &cancel);
            if let Err(error) = result {
                if !matches!(
                    error.as_str(),
                    "STALE_WORKER_EPOCH" | "CLOUD_PAUSED" | "RUNTIME_SHUTDOWN"
                ) && !runtime.exit.load(Ordering::Relaxed)
                {
                    runtime.error(&sid, &error, true);
                }
            }
        });
        *slot = Some(CloudTask {
            session: task_session,
            stop,
            thread,
        });
        Ok(())
    }
    fn spawn_cloud_retry(self: &Arc<Self>, sid: String) -> Result<(), String> {
        self.spawn_cloud_retry_with(sid, || Ok(()))
    }
    fn cloud_resume(&self, sid: &str, rid: &str) -> Result<CloudResume, String> {
        let session = self.session(sid)?;
        let prefix = format!("{rid}_cloud_");
        let mut final_end = 0;
        let mut next_ordinal = 0;
        let mut partial: Option<(u64, u64)> = None;
        for segment in session["segments"].as_array().into_iter().flatten() {
            if segment["runId"] != rid {
                continue;
            }
            let Some(ordinal) = segment["id"]
                .as_str()
                .and_then(|id| id.strip_prefix(&prefix))
                .and_then(|ordinal| ordinal.parse::<u64>().ok())
            else {
                continue;
            };
            if segment["final"] == true {
                final_end = final_end.max(segment["endSample"].as_u64().unwrap_or(0));
                next_ordinal = next_ordinal.max(ordinal.saturating_add(1));
            } else if partial
                .as_ref()
                .is_none_or(|(current, _)| ordinal >= *current)
            {
                partial = Some((
                    ordinal,
                    segment["startSample"].as_u64().unwrap_or(final_end),
                ));
            }
        }
        if let Some((ordinal, start)) = partial {
            return Ok(CloudResume {
                cursor: start.max(final_end),
                ordinal,
            });
        }
        let pending = self
            .jobs()?
            .into_iter()
            .filter(|job| job.session == sid && job.run == rid)
            .map(|job| job.start)
            .min()
            .unwrap_or(final_end);
        Ok(CloudResume {
            cursor: final_end.max(pending),
            ordinal: next_ordinal,
        })
    }
    fn finish_cloud_attempt(&self, sid: &str, rid: &str, result: Result<(), String>) {
        match result {
            Ok(()) => {
                if let Ok(jobs) = self.jobs() {
                    for job in jobs
                        .into_iter()
                        .filter(|job| job.session == sid && job.run == rid)
                    {
                        let _ = fs::remove_file(self.job_path(&job));
                    }
                }
                if let Ok(mut partial) = self.partial.lock() {
                    partial.retain(|_, job| job.session != sid || job.run != rid);
                }
                let has_pending = self
                    .jobs()
                    .unwrap_or_default()
                    .iter()
                    .any(|job| job.session == sid && self.job_is_cloud(job).unwrap_or(false));
                if !has_pending {
                    let _ = self.update(sid, |session| {
                        session["inferenceState"] = json!("ready");
                        if session["recordingState"] != "error" {
                            session["error"] = Value::Null;
                        }
                        Ok(())
                    });
                }
            }
            Err(error) => {
                if error == "CLOUD_PAUSED" {
                    let _ = self.update(sid, |session| {
                        session["inferenceState"] = json!("paused");
                        session["error"] = Value::Null;
                        Ok(())
                    });
                } else if !matches!(error.as_str(), "STALE_WORKER_EPOCH" | "RUNTIME_SHUTDOWN")
                    && !self.exit.load(Ordering::Relaxed)
                {
                    self.error(sid, &error, true);
                }
            }
        }
    }
    fn run_cloud_retries(
        self: &Arc<Self>,
        sid: &str,
        epoch: u64,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut runs = Vec::new();
        for job in self.jobs()?.into_iter().filter(|job| job.session == sid) {
            if self.job_is_cloud(&job)? && !runs.contains(&job.run) {
                runs.push(job.run);
            }
        }
        for rid in runs {
            if self.exit.load(Ordering::Relaxed) {
                return Err("RUNTIME_SHUTDOWN".into());
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("CLOUD_PAUSED".into());
            }
            self.cloud_upload_allowed(epoch)?;
            let (key, language, terms) = self.soniox_configuration(sid)?;
            let client = crate::soniox::Client::connect(&key, &language, &terms)?;
            let resume = self.cloud_resume(sid, &rid)?;
            let result = cloud_run(self, sid, &rid, client, resume, epoch, cancel);
            let failed = result.as_ref().err().cloned();
            self.finish_cloud_attempt(sid, &rid, result);
            if let Some(error) = failed {
                return Err(error);
            }
        }
        if self
            .jobs()?
            .iter()
            .all(|job| job.session != sid || !self.job_is_cloud(job).unwrap_or(false))
        {
            let _ = self.update(sid, |session| {
                session["inferenceState"] = json!("ready");
                Ok(())
            });
        }
        Ok(())
    }
    fn start(self: &Arc<Self>, sid: &str, source: &str) -> Result<(), String> {
        if !["microphone", "system"].contains(&source) {
            return Err("请选择麦克风或系统声音".into());
        }
        let mut active = self.capture.lock().map_err(|_| "录音控制器不可用")?;
        if self.model_installing.load(Ordering::SeqCst) {
            return Err("本地模型正在安装，请完成后再开始录音".into());
        }
        if active.as_ref().is_some_and(|r| r.thread.is_finished()) {
            if let Some(r) = active.take() {
                let _ = r.thread.join();
            }
        }
        if let Some(r) = active.as_ref() {
            if r.session == sid {
                return Ok(());
            }
            return Err("另一门课程正在录音，请先停止".into());
        }
        self.reap_cloud();
        let settings = self.value()?["settings"].clone();
        let cloud_client = if settings["engine"] == "soniox" {
            if self
                .cloud
                .lock()
                .map_err(|_| "云端转写控制器不可用")?
                .is_some()
            {
                return Err("另一项 Soniox 转写正在进行，请稍后再试".into());
            }
            let (key, language, terms) = self.soniox_configuration(sid)?;
            Some(crate::soniox::Client::connect(&key, &language, &terms)?)
        } else {
            if settings["engine"] == "whisper" {
                return Err(WHISPER_REALTIME_UNAVAILABLE.into());
            }
            let info = self.info();
            if info["modelReady"] != true {
                return Err(info["modelError"]
                    .as_str()
                    .unwrap_or("本地模型正在准备；模型就绪后即可开始录音")
                    .to_owned());
            }
            None
        };
        let rid = self.add_run(sid, source)?;
        let result = if source == "system" {
            capture::system(self.helper())
        } else {
            capture::microphone()
        };
        let input = match result {
            Ok(v) => v,
            Err(e) => {
                let _ = self.close_run(sid, &rid, 0, "interrupted", Some(&e));
                return Err(e);
            }
        };
        let archiver = match Archiver::new(self.clone(), sid.to_owned(), rid.clone(), input.rate) {
            Ok(mut archiver) => {
                if source == "microphone" {
                    archiver.watch_for_muted_input();
                }
                archiver
            }
            Err(error) => {
                input.handle.stop();
                let _ = self.close_run(sid, &rid, 0, "interrupted", Some(&error));
                return Err(error);
            }
        };
        if let Some(client) = cloud_client {
            if let Err(error) = self.spawn_cloud_live(sid.to_owned(), rid.clone(), client) {
                input.handle.stop();
                let _ = self.close_run(sid, &rid, 0, "interrupted", Some(&error));
                return Err(error);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let runtime = self.clone();
        let session = sid.to_owned();
        self.update(sid, |s| {
            s["recordingState"] = json!("recording");
            Ok(())
        })?;
        let thread = thread::spawn(move || {
            let mut archiver = archiver;
            let mut failed = None;
            let cursor = input.captured.clone();
            while !flag.load(Ordering::Relaxed) && !runtime.exit.load(Ordering::Relaxed) {
                for (start, end) in input.gaps.try_iter() {
                    let _ = archiver.gap(start, end, "采集队列音频缺口");
                }
                for notice in input.notices.try_iter() {
                    let message = match notice {
                        capture::MicrophoneNotice::Lost(name) => {
                            archiver.gap_reason = Some("麦克风切换");
                            format!("麦克风“{name}”已断开，正在切换到其他输入设备，录音会继续")
                        }
                        capture::MicrophoneNotice::Switched(name) => {
                            format!("已切换到“{name}”继续录音，断开期间记为音频缺口")
                        }
                    };
                    let _ = runtime.update(&session, |s| {
                        s["error"] = json!(message);
                        Ok(())
                    });
                }
                if let Ok(e) = input.errors.try_recv() {
                    failed = Some(e);
                    break;
                }
                match input.frames.recv_timeout(Duration::from_millis(100)) {
                    Ok(frame) => {
                        if let Err(e) = archiver.frame(frame) {
                            failed = Some(e);
                            break;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        failed = Some("音频源已断开".into());
                        break;
                    }
                    _ => {}
                }
            }
            input.handle.stop();
            for frame in input.frames.try_iter() {
                if let Err(e) = archiver.frame(frame) {
                    failed = Some(e);
                    break;
                }
            }
            for (start, end) in input.gaps.try_iter() {
                let _ = archiver.gap(start, end, "采集队列音频缺口");
            }
            let final_cursor = cursor.load(Ordering::SeqCst);
            if final_cursor > archiver.input_cursor {
                if let Err(e) = archiver.frame(capture::Frame {
                    start: final_cursor,
                    samples: Vec::new(),
                }) {
                    failed = Some(e);
                }
            }
            if let Err(e) = archiver.finish() {
                failed = Some(e);
            }
            if let Err(error) = runtime.close_run(
                &session,
                &rid,
                archiver.samples,
                if failed.is_some() {
                    "interrupted"
                } else {
                    "closed"
                },
                failed.as_deref(),
            ) {
                runtime.error(&session, &format!("录音结束时保存失败：{error}"), false);
            }
        });
        *active = Some(Recording {
            session: sid.to_owned(),
            stop,
            thread,
        });
        Ok(())
    }
    fn pause(&self, sid: &str) -> Result<(), String> {
        if self.session(sid)?["recordingState"] != "recording" {
            return Err("当前课程没有正在进行的录音".into());
        }
        self.stop(sid)?;
        self.update(sid, |session| {
            // close_run records a capture failure as "error"; pausing must not hide it.
            if session["recordingState"] != "error" {
                session["recordingState"] = json!("paused");
                session["error"] = Value::Null;
            }
            Ok(())
        })?;
        Ok(())
    }
    fn stop(&self, sid: &str) -> Result<(), String> {
        let mut capture = self.capture.lock().map_err(|_| "录音控制器不可用")?;
        if let Some(c) = capture.as_ref() {
            if c.session != sid {
                return Err("当前录音属于另一门课程".into());
            }
        }
        if let Some(c) = capture.take() {
            self.update(sid, |s| {
                s["recordingState"] = json!("stopping");
                Ok(())
            })?;
            c.stop.store(true, Ordering::SeqCst);
            c.thread
                .join()
                .map_err(|_| "录音线程中断，已保存的音频可在重启后恢复")?;
        } else if self.session(sid)?["recordingState"] == "paused" {
            self.update(sid, |session| {
                session["recordingState"] = json!("stopped");
                Ok(())
            })?;
        }
        Ok(())
    }
    fn close_run(
        &self,
        sid: &str,
        rid: &str,
        samples: u64,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        // A device permission failure can close a zero-length run before the
        // archiver opens it. Preserve a valid empty recording for export/reopen.
        if samples == 0 {
            if let Ok(path) = self.run_path(sid, rid) {
                if !path.exists() {
                    let _ = atomic_write(&path, &[]);
                }
            }
        }
        self.update(sid, |s| {
            if let Some(r) = s["runs"]
                .as_array_mut()
                .and_then(|r| r.iter_mut().find(|r| r["id"] == rid))
            {
                r["samples"] = json!(samples);
                r["state"] = json!(state);
                r["endedAt"] = json!(now());
            }
            clamp_run_ranges(s, rid, samples);
            s["recordingState"] = json!(if error.is_some() { "error" } else { "stopped" });
            if let Some(e) = error {
                s["error"] = json!(e);
            }
            Ok(())
        })?;
        // The last sentence was held back while the run was open.
        if let Some(segment_id) = self
            .snapshot()
            .ok()
            .and_then(|state| auto_polish_run_tail(&state, sid, rid))
        {
            self.schedule_auto_polish(sid, &segment_id);
        }
        Ok(())
    }
    fn import_audio(self: &Arc<Self>, sid: &str, path: &Path) -> Result<(), String> {
        if self.capture.lock().unwrap().is_some() {
            return Err("请结束当前录音后导入音频".into());
        }
        let mut reader =
            hound::WavReader::open(path).map_err(|e| format!("请选择有效的 WAV 音频：{e}"))?;
        let spec = reader.spec();
        if spec.channels == 0
            || spec.channels > 8
            || spec.sample_rate < 8000
            || spec.sample_rate > 192000
        {
            return Err("支持 8–192 kHz、最多 8 声道的 WAV 文件".into());
        }
        if reader.duration() as u64 / spec.sample_rate as u64 > 12 * 3600 {
            return Err("单次导入最长 12 小时".into());
        }
        let rid = self.add_run(sid, "import")?;
        let result = (|| {
            let mut a = Archiver::new(self.clone(), sid.to_owned(), rid.clone(), spec.sample_rate)?;
            let mut buffer = Vec::new();
            let mut input_pos = 0u64;
            let samples: Box<dyn Iterator<Item = Result<f32, hound::Error>> + '_> =
                match spec.sample_format {
                    hound::SampleFormat::Float => Box::new(reader.samples::<f32>()),
                    hound::SampleFormat::Int => {
                        let scale = 2f32.powi(spec.bits_per_sample as i32 - 1);
                        Box::new(
                            reader
                                .samples::<i32>()
                                .map(move |s| s.map(|x| x as f32 / scale)),
                        )
                    }
                };
            let mut frame = Vec::new();
            for sample in samples {
                frame.push(sample.map_err(|e| e.to_string())?);
                if frame.len() == spec.channels as usize {
                    buffer.push(frame.iter().sum::<f32>() / spec.channels as f32);
                    frame.clear();
                }
                if buffer.len() >= spec.sample_rate as usize {
                    let n = buffer.len();
                    a.frame(capture::Frame {
                        start: input_pos,
                        samples: std::mem::take(&mut buffer),
                    })?;
                    input_pos += n as u64;
                }
            }
            if !buffer.is_empty() {
                a.frame(capture::Frame {
                    start: input_pos,
                    samples: buffer,
                })?;
            }
            a.finish()?;
            Ok::<_, String>(a.samples)
        })();
        match result {
            Ok(samples) => {
                self.close_run(sid, &rid, samples, "closed", None)?;
                if self.run_engine(sid, &rid)?.as_deref() == Some("soniox") {
                    self.update(sid, |session| {
                        session["inferenceState"] = json!("catching_up");
                        Ok(())
                    })?;
                    self.soniox_configuration(sid)?;
                    self.spawn_cloud_retry(sid.to_owned())?;
                }
                Ok(())
            }
            Err(e) => {
                let samples = self
                    .run_path(sid, &rid)?
                    .metadata()
                    .map(|m| m.len() / 2)
                    .unwrap_or(0);
                let _ = self.close_run(sid, &rid, samples, "interrupted", Some(&e));
                Err(e)
            }
        }
    }
    fn jobs(&self) -> Result<Vec<Job>, String> {
        let mut jobs: Vec<Job> = Vec::new();
        for item in fs::read_dir(self.root.join("jobs")).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let parsed = (|| -> Result<Job, String> {
                let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                if !meta.is_file() || meta.len() > 65536 {
                    return Err("任务文件格式无效".into());
                }
                let job: Job = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|_| "任务内容损坏")?;
                if !crate::domain::safe_id(&job.id)
                    || !crate::domain::safe_id(&job.session)
                    || !crate::domain::safe_id(&job.run)
                    || job.end <= job.start
                    || job.end - job.start > 960000
                    || !job.final_result
                {
                    return Err("任务范围无效".into());
                }
                Ok(job)
            })();
            match parsed {
                Ok(job) => jobs.push(job),
                Err(_) => {
                    let quarantine = self.root.join("recovery/quarantined-jobs");
                    fs::create_dir_all(&quarantine).map_err(|e| e.to_string())?;
                    fs::rename(
                        &path,
                        quarantine.join(format!(
                            "{}-{}",
                            uid(),
                            path.file_name().unwrap().to_string_lossy()
                        )),
                    )
                    .map_err(|e| e.to_string())?;
                    if let Some(sid) = self.snapshot()?.selected_session_id {
                        self.error(
                            &sid,
                            "一个转写任务已损坏，已隔离保存；其他录音与转写可继续。",
                            true,
                        );
                    }
                }
            }
        }
        jobs.sort_by_key(|j| (j.session.clone(), j.run.clone(), j.start));
        Ok(jobs)
    }
    fn job_path(&self, j: &Job) -> PathBuf {
        self.root.join("jobs").join(format!("{}.json", j.id))
    }
    fn save_job(&self, j: &Job) -> Result<(), String> {
        atomic_write(
            &self.job_path(j),
            &serde_json::to_vec(j).map_err(|e| e.to_string())?,
        )
    }
    fn enqueue(&self, j: Job) -> Result<(), String> {
        // Publish the durable audio boundary before any worker can commit a hypothesis.
        // This also covers final windows shorter than the periodic one-second checkpoint.
        self.update(&j.session, |s| {
            let run = s["runs"]
                .as_array_mut()
                .and_then(|runs| runs.iter_mut().find(|r| r["id"] == j.run))
                .ok_or("录音不存在")?;
            run["samples"] = json!(run["samples"].as_u64().unwrap_or(0).max(j.end));
            Ok(())
        })?;
        if j.final_result {
            self.partial.lock().unwrap().remove(&j.id);
            self.save_job(&j)?;
        } else {
            let mut p = self.partial.lock().unwrap();
            p.insert(j.id.clone(), j);
            if p.len() > 16 {
                if let Some(key) = p.keys().next().cloned() {
                    p.remove(&key);
                }
            }
        }
        Ok(())
    }
    fn recover(&self) -> Result<(), String> {
        let mut jobs = self.jobs()?;
        let sessions = self.value()?["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for s in sessions {
            if s["mode"] == "demo" {
                continue;
            }
            let sid = field(&s, "id")?;
            for run in s["runs"].as_array().into_iter().flatten() {
                let rid = field(run, "id")?;
                let path = self.run_path(&sid, &rid)?;
                if !path.exists() {
                    if run["samples"] == 0 && run["state"] == "interrupted" {
                        atomic_write(&path, &[])?;
                        continue;
                    }
                    self.error(&sid, "部分音频文件缺失，请从课程备份恢复", false);
                    continue;
                }
                let len = path.metadata().map_err(|e| e.to_string())?.len();
                let samples = len / 2;
                if len % 2 == 1 {
                    OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .map_err(|e| e.to_string())?
                        .set_len(samples * 2)
                        .map_err(|e| e.to_string())?;
                }
                if ["recording", "starting"].contains(&run["state"].as_str().unwrap_or("")) {
                    // Jobs queued past the durable audio would fail read_exact forever.
                    for job in jobs
                        .iter_mut()
                        .filter(|j| j.session == sid && j.run == rid && j.end > samples)
                    {
                        if job.start >= samples {
                            fs::remove_file(self.job_path(job)).map_err(|e| e.to_string())?;
                            job.end = job.start;
                        } else {
                            job.end = samples;
                            self.save_job(job)?;
                        }
                    }
                    let final_end = s["segments"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|p| p["runId"] == rid && p["final"] == true)
                        .map(|p| p["endSample"].as_u64().unwrap_or(0))
                        .max()
                        .unwrap_or(0);
                    let queued_end = jobs
                        .iter()
                        .filter(|j| j.session == sid && j.run == rid && j.end > j.start)
                        .map(|j| j.end)
                        .max()
                        .unwrap_or(0);
                    let mut start = final_end.max(queued_end);
                    while start < samples {
                        let end = (start + QWEN_MAX_SEGMENT_SAMPLES).min(samples);
                        self.enqueue(Job {
                            id: format!("{rid}_{start}"),
                            session: sid.clone(),
                            run: rid.clone(),
                            start,
                            end,
                            final_result: true,
                            attempts: 0,
                            failed: false,
                        })?;
                        start = end;
                    }
                    if let Err(error) = self.close_run(
                        &sid,
                        &rid,
                        samples,
                        "interrupted",
                        Some("上次录音意外中断，已恢复可读音频；待处理转写将继续"),
                    ) {
                        self.error(&sid, &format!("中断的录音无法恢复：{error}"), false);
                    }
                }
            }
        }
        Ok(())
    }
    fn wav(&self, job: &Job) -> Result<Vec<u8>, String> {
        if job.end <= job.start || job.end - job.start > 16000 * 60 {
            return Err("转写范围无效".into());
        }
        let mut f =
            File::open(self.run_path(&job.session, &job.run)?).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(job.start * 2))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; ((job.end - job.start) * 2) as usize];
        f.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        Ok(wav_bytes(&bytes))
    }
    pub fn audio_data(&self, sid: &str, rid: &str, start: u64, end: u64) -> Result<String, String> {
        let s = self.session(sid)?;
        if !s["runs"]
            .as_array()
            .is_some_and(|r| r.iter().any(|r| r["id"] == rid))
        {
            return Err("录音不属于当前课程".into());
        }
        if s["recordingState"] == "recording"
            && s["runs"].as_array().is_some_and(|r| {
                r.iter()
                    .any(|r| r["state"] == "recording" && r["source"] == "system")
            })
        {
            return Err("系统声音录制期间请停止录音后回放，避免回授".into());
        }
        if end <= start || end - start > 960000 {
            return Err("逐段回放支持最多 60 秒音频".into());
        }
        let mut file = File::open(self.run_path(sid, rid)?).map_err(|e| e.to_string())?;
        let total = file.metadata().map_err(|e| e.to_string())?.len() / 2;
        let end = end.min(total);
        if start >= end {
            return Err("该片段的音频尚未保存".into());
        }
        file.seek(SeekFrom::Start(start * 2))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; ((end - start) * 2) as usize];
        file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        Ok(format!(
            "data:audio/wav;base64,{}",
            STANDARD.encode(wav_bytes(&bytes))
        ))
    }
    fn export(&self, c: &Value) -> Result<(), String> {
        let sid = field(c, "sessionId")?;
        let value = self.session(&sid)?;
        let session = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let path = PathBuf::from(field(c, "path")?);
        match c["format"].as_str().unwrap_or("") {
            "markdown" => atomic_write(&path, crate::archive::export_markdown(&session).as_bytes()),
            "html" => atomic_write(&path, crate::archive::export_html(&session).as_bytes()),
            "lecture" => crate::archive::export_lecture(&session, &self.root.join("audio"), &path),
            "wav" => {
                let temp = path.with_extension(format!("{}.part", uid()));
                let written = self.write_wav(&sid, &value, &temp, &path);
                if written.is_err() {
                    let _ = fs::remove_file(&temp);
                }
                written?;
                atomic_write(
                    &path.with_extension("gaps.json"),
                    serde_json::to_string_pretty(
                        &json!({"sampleRate":16000,"runs":value["runs"],"gaps":value["gaps"]}),
                    )
                    .unwrap()
                    .as_bytes(),
                )
            }
            _ => Err("导出格式无效".into()),
        }
    }
    fn write_wav(&self, sid: &str, value: &Value, temp: &Path, path: &Path) -> Result<(), String> {
        let mut writer = hound::WavWriter::create(
            temp,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(|e| e.to_string())?;
        for r in value["runs"].as_array().into_iter().flatten() {
            let rid = field(r, "id")?;
            let mut f = File::open(self.run_path(sid, &rid)?).map_err(|e| e.to_string())?;
            let mut left = r["samples"].as_u64().unwrap_or(0) * 2;
            let mut buf = vec![0; 32768];
            while left > 0 {
                let n = left.min(buf.len() as u64) as usize;
                f.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
                for b in buf[..n].chunks_exact(2) {
                    writer
                        .write_sample(i16::from_le_bytes([b[0], b[1]]))
                        .map_err(|e| e.to_string())?;
                }
                left -= n as u64;
            }
        }
        writer.finalize().map_err(|e| e.to_string())?;
        File::open(temp)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(temp, path).map_err(|e| e.to_string())
    }
    pub fn shutdown(&self) {
        self.exit.store(true, Ordering::SeqCst);
        if let Ok(mut c) = self.capture.lock() {
            if let Some(r) = c.take() {
                r.stop.store(true, Ordering::SeqCst);
                let _ = r.thread.join();
            }
        }
        if let Some(mut child) = self.model_child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(mut cloud) = self.cloud.lock() {
            if let Some(task) = cloud.take() {
                task.stop.store(true, Ordering::SeqCst);
                let _ = task.thread.join();
            }
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
fn cloud_run<C: CloudStream>(
    runtime: &Runtime,
    sid: &str,
    rid: &str,
    mut client: C,
    resume: CloudResume,
    epoch: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let path = runtime.run_path(sid, rid)?;
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut cursor = resume.cursor;
    file.seek(SeekFrom::Start(cursor.saturating_mul(2)))
        .map_err(|error| error.to_string())?;
    let mut next_send: Option<Instant> = None;
    runtime.update(sid, |session| {
        session["inferenceState"] = json!(if session["recordingState"] == "recording" {
            "running"
        } else {
            "catching_up"
        });
        Ok(())
    })?;
    loop {
        if runtime.exit.load(Ordering::Relaxed) {
            return Err("RUNTIME_SHUTDOWN".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("CLOUD_PAUSED".into());
        }
        runtime.cloud_upload_allowed(epoch)?;
        let session = runtime.session(sid)?;
        let run = session["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|run| run["id"] == rid))
            .ok_or("录音不存在")?;
        if run["engine"] != "soniox" {
            return Err("录音的识别引擎不匹配".into());
        }
        let boundary = run["samples"].as_u64().unwrap_or(0);
        if cursor < boundary {
            let count = (boundary - cursor).min(3200);
            if let Some(due) = next_send {
                while Instant::now() < due {
                    if runtime.exit.load(Ordering::Relaxed) {
                        return Err("RUNTIME_SHUTDOWN".into());
                    }
                    if cancel.load(Ordering::Relaxed) {
                        return Err("CLOUD_PAUSED".into());
                    }
                    thread::sleep(
                        due.saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(25)),
                    );
                }
            }
            let mut bytes = vec![0; count.saturating_mul(2) as usize];
            file.read_exact(&mut bytes)
                .map_err(|_| "已保存音频暂时不可读")?;
            runtime.cloud_upload_allowed(epoch)?;
            client.send_audio(&bytes)?;
            cursor += count;
            next_send = Some(Instant::now() + Duration::from_secs_f64(count as f64 / 16000.0));
            for update in client.poll()? {
                commit_cloud_update(runtime, sid, rid, &resume, update, epoch)?;
            }
            continue;
        }
        if matches!(run["state"].as_str(), Some("recording" | "starting")) {
            for update in client.poll()? {
                commit_cloud_update(runtime, sid, rid, &resume, update, epoch)?;
            }
            thread::sleep(Duration::from_millis(35));
            continue;
        }
        break;
    }
    runtime.cloud_upload_allowed(epoch)?;
    client.finish_input()?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !client.finished() && Instant::now() < deadline {
        if runtime.exit.load(Ordering::Relaxed) {
            return Err("RUNTIME_SHUTDOWN".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("CLOUD_PAUSED".into());
        }
        for update in client.poll()? {
            commit_cloud_update(runtime, sid, rid, &resume, update, epoch)?;
        }
    }
    if !client.finished() {
        return Err("Soniox 最终结果等待超时；音频已保留，可稍后重试".into());
    }
    Ok(())
}
fn commit_cloud_update(
    runtime: &Runtime,
    sid: &str,
    rid: &str,
    resume: &CloudResume,
    update: crate::soniox::Update,
    epoch: u64,
) -> Result<(), String> {
    if runtime.snapshot()?.worker_epoch != epoch {
        return Err("STALE_WORKER_EPOCH".into());
    }
    let session = runtime.session(sid)?;
    let samples = session["runs"]
        .as_array()
        .and_then(|runs| runs.iter().find(|run| run["id"] == rid))
        .and_then(|run| run["samples"].as_u64())
        .ok_or("录音不存在")?;
    let ordinal = resume
        .ordinal
        .checked_add(update.index)
        .ok_or("Soniox 片段编号溢出")?;
    let segment_id = format!("{rid}_cloud_{ordinal}");
    let existing = session["segments"]
        .as_array()
        .and_then(|segments| segments.iter().find(|segment| segment["id"] == segment_id));
    if existing.is_some_and(|segment| segment["final"] == true) {
        return Ok(());
    }
    let proposed_start = resume
        .cursor
        .saturating_add(update.start_sample)
        .min(samples);
    let start = existing
        .and_then(|segment| segment["startSample"].as_u64())
        .unwrap_or(proposed_start);
    let proposed_end = resume
        .cursor
        .saturating_add(update.end_sample)
        .min(samples)
        .max(start);
    let end = if update.final_result {
        proposed_end
    } else {
        existing
            .and_then(|segment| segment["endSample"].as_u64())
            .unwrap_or(start)
            .max(proposed_end)
    };
    let revision = existing
        .and_then(|segment| segment["machineRevision"].as_u64())
        .unwrap_or(0)
        .saturating_add(1);
    runtime
        .store
        .lock()
        .map_err(|_| "数据写入器不可用")?
        .apply(json!({"type":"machine","commandId":uid(),"sessionId":sid,"segmentId":segment_id,"runId":rid,"startSample":start,"endSample":end,"text":update.text,"revision":revision,"workerEpoch":epoch,"final":update.final_result,"allowFinalRangeShrink":update.final_result}))?;
    if update.final_result {
        runtime.schedule_auto_polish(sid, &segment_id);
    }
    Ok(())
}
/// RMS a 20 ms recorder frame must exceed to count as speech.
const SPEECH_RMS: f64 = 0.009;
fn rms(samples: impl Iterator<Item = i16>) -> f64 {
    let (sum, count) = samples.fold((0f64, 0usize), |(sum, count), sample| {
        (sum + (sample as f64 / 32768.).powi(2), count + 1)
    });
    (sum / count.max(1) as f64).sqrt()
}
/// 20 ms frames of speech a segment needs before it is worth sending to the model.
const MIN_SPEECH_FRAMES: usize = 10;
/// Near-silent audio makes the model hallucinate (often words from the vocabulary), so it is
/// skipped unless at least 200 ms is loud enough for the recorder to have called it speech;
/// a lone click or breath does not qualify.
fn has_speech(pcm: &[u8]) -> bool {
    pcm.chunks(640)
        .filter(|frame| {
            rms(frame
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])))
                > SPEECH_RMS
        })
        .nth(MIN_SPEECH_FRAMES - 1)
        .is_some()
}
/// Keeps segment and note anchors inside a run whose durable audio is shorter than published.
fn clamp_run_ranges(session: &mut Value, rid: &str, samples: u64) {
    for segment in session["segments"].as_array_mut().into_iter().flatten() {
        if segment["runId"] == rid {
            for key in ["startSample", "endSample"] {
                if segment[key].as_u64().is_some_and(|value| value > samples) {
                    segment[key] = json!(samples);
                }
            }
        }
    }
    for note in session["notes"].as_array_mut().into_iter().flatten() {
        if note["runId"] == rid && note["sample"].as_u64().is_some_and(|value| value > samples) {
            note["sample"] = json!(samples);
        }
    }
}
fn wav_bytes(pcm: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(pcm.len() + 44);
    b.extend(b"RIFF");
    b.extend(((pcm.len() + 36) as u32).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(16000u32.to_le_bytes());
    b.extend(32000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((pcm.len() as u32).to_le_bytes());
    b.extend(pcm);
    b
}
struct Archiver {
    runtime: Arc<Runtime>,
    sid: String,
    rid: String,
    file: File,
    resampler: capture::Resampler,
    input_cursor: u64,
    input_rate: u32,
    samples: u64,
    segment_start: u64,
    voiced: bool,
    silence: u64,
    last_partial: u64,
    last_sync: u64,
    /// Digital-zero samples seen since recording began, while still watching for a muted input.
    muted_samples: Option<u64>,
    /// Why the next jump in the input timeline happened, when the source said so.
    gap_reason: Option<&'static str>,
}
/// A microphone the OS will not let us hear delivers exact zeros instead of an error
/// (missing permission, or on macOS a build without the audio-input entitlement).
const MUTED_INPUT_SAMPLES: u64 = 3 * 16_000;
const MUTED_MICROPHONE: &str = if cfg!(target_os = "macos") {
    "麦克风没有传来任何声音。请在「系统设置 → 隐私与安全性 → 麦克风」中允许“随堂”，然后重新开始录音"
} else {
    "麦克风没有传来任何声音。请在「设置 → 隐私和安全性 → 麦克风」中打开“允许桌面应用访问麦克风”，并确认麦克风没有被静音"
};
impl Archiver {
    fn new(runtime: Arc<Runtime>, sid: String, rid: String, rate: u32) -> Result<Self, String> {
        let path = runtime.run_path(&sid, &rid)?;
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            runtime,
            sid,
            rid,
            file,
            resampler: capture::Resampler::new(rate),
            input_cursor: 0,
            input_rate: rate,
            samples: 0,
            segment_start: 0,
            voiced: false,
            silence: 0,
            last_partial: 0,
            last_sync: 0,
            muted_samples: None,
            gap_reason: None,
        })
    }
    fn watch_for_muted_input(&mut self) {
        self.muted_samples = Some(0);
    }
    fn gap(&self, start: u64, end: u64, reason: &str) -> Result<(), String> {
        let start = start * 16000 / self.input_rate as u64;
        let end = end * 16000 / self.input_rate as u64;
        self.runtime.update(&self.sid, |s| {
            let gaps = s["gaps"].as_array_mut().ok_or("无效的缺口列表")?;
            if !gaps.iter().any(|g| {
                g["runId"] == self.rid && g["startSample"] == start && g["endSample"] == end
            }) {
                gaps.push(
                    json!({"runId":self.rid,"startSample":start,"endSample":end,"reason":reason}),
                );
            }
            s["error"] = json!("检测到音频缺口，已保留时间位置和缺口记录");
            Ok(())
        })?;
        Ok(())
    }
    fn frame(&mut self, frame: capture::Frame) -> Result<(), String> {
        if frame.start < self.input_cursor {
            return Err("音频采样顺序无效".into());
        }
        if frame.start > self.input_cursor {
            let lost = (frame.start - self.input_cursor) * 16000 / self.input_rate as u64;
            let start = self.samples;
            let reason = self.gap_reason.take().unwrap_or("采集队列溢出");
            self.runtime.update(&self.sid,|s|{s["gaps"].as_array_mut().ok_or("无效的缺口列表")?.push(json!({"runId":self.rid,"startSample":start,"endSample":start+lost,"reason":reason}));if reason == "采集队列溢出" {s["error"]=json!("检测到音频缺口，已保留时间位置和缺口记录");}Ok(())})?;
            // Padding is silence we wrote, not evidence of a muted microphone.
            let watching = self.muted_samples.take();
            let mut remaining = lost;
            while remaining > 0 {
                let n = remaining.min(16000) as usize;
                self.pcm(&vec![0; n])?;
                remaining -= n as u64;
            }
            self.muted_samples = watching;
        }
        self.input_cursor = frame.start + frame.samples.len() as u64;
        let pcm = self.resampler.push(&frame.samples);
        for part in pcm.chunks(320) {
            self.pcm(part)?;
        }
        Ok(())
    }
    fn pcm(&mut self, pcm: &[i16]) -> Result<(), String> {
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("录音保存失败：{e}"))?;
        self.samples += pcm.len() as u64;
        if let Some(muted) = self.muted_samples {
            let muted = muted + pcm.len() as u64;
            self.muted_samples = if pcm.iter().any(|&sample| sample != 0) {
                None
            } else if muted >= MUTED_INPUT_SAMPLES {
                self.runtime.update(&self.sid, |s| {
                    s["error"] = json!(MUTED_MICROPHONE);
                    Ok(())
                })?;
                None
            } else {
                Some(muted)
            };
        }
        if rms(pcm.iter().copied()) > SPEECH_RMS {
            self.voiced = true;
            self.silence = 0;
        } else {
            self.silence += pcm.len() as u64;
        }
        let length = self.samples - self.segment_start;
        let final_now = length >= QWEN_MAX_SEGMENT_SAMPLES
            || (self.voiced && self.silence >= 9600 && length >= 19200);
        if final_now {
            self.finalize_segment()?;
        } else if self.voiced && length >= 32000 && self.samples - self.last_partial >= 32000 {
            // enqueue publishes run.samples; it must never run ahead of what survives power loss.
            self.file
                .sync_all()
                .map_err(|e| format!("录音写盘失败：{e}"))?;
            self.runtime.enqueue(self.job(false))?;
            self.last_partial = self.samples;
        }
        if self.samples - self.last_sync >= 16000 {
            self.file
                .sync_all()
                .map_err(|e| format!("录音写盘失败：{e}"))?;
            self.last_sync = self.samples;
            let count = self.samples;
            let rid = &self.rid;
            self.runtime.update(&self.sid, |s| {
                if let Some(r) = s["runs"]
                    .as_array_mut()
                    .and_then(|r| r.iter_mut().find(|r| r["id"] == rid.as_str()))
                {
                    r["samples"] = json!(count);
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    fn job(&self, final_result: bool) -> Job {
        Job {
            id: format!("{}_{}", self.rid, self.segment_start),
            session: self.sid.clone(),
            run: self.rid.clone(),
            start: self.segment_start,
            end: self.samples,
            final_result,
            attempts: 0,
            failed: false,
        }
    }
    fn finalize_segment(&mut self) -> Result<(), String> {
        if self.samples > self.segment_start {
            self.file.sync_all().map_err(|e| e.to_string())?;
            self.runtime.enqueue(self.job(true))?;
            self.segment_start = self.samples;
            self.voiced = false;
            self.silence = 0;
            self.last_partial = self.samples;
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        self.finalize_segment()?;
        self.file.sync_all().map_err(|e| e.to_string())
    }
}
struct Model {
    settings: Value,
    /// Whether llama-server was allowed to use a GPU backend.
    gpu: bool,
    child: Arc<Mutex<Option<Child>>>,
    port: u16,
    key: String,
    client: reqwest::blocking::Client,
}
fn model_files_present(settings: &Value) -> bool {
    ["executable", "modelPath"]
        .iter()
        .all(|key| Path::new(settings[*key].as_str().unwrap_or("")).is_file())
        && (settings["engine"] == "whisper"
            || Path::new(settings["mmprojPath"].as_str().unwrap_or("")).is_file())
}
fn vocabulary_pair(entry: &str) -> (Option<&str>, &str) {
    let trimmed = entry.trim();
    match trimmed.split_once('→').or_else(|| trimmed.split_once("=>")) {
        Some((heard, preferred)) if !heard.trim().is_empty() && !preferred.trim().is_empty() => {
            (Some(heard.trim()), preferred.trim())
        }
        _ => (None, trimmed),
    }
}
fn vocabulary_terms(settings: &Value) -> Vec<String> {
    let mut terms = Vec::new();
    for entry in settings["customVocabulary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let (_, preferred) = vocabulary_pair(entry);
        if !preferred.is_empty()
            && !terms
                .iter()
                .any(|term: &String| term.eq_ignore_ascii_case(preferred))
        {
            terms.push(preferred.to_owned());
        }
    }
    terms
}

fn vocabulary_identity(entry: &str) -> String {
    let (heard, preferred) = vocabulary_pair(entry);
    match heard {
        Some(heard) => format!("mapping:{}", heard.to_lowercase()),
        None => format!("term:{}", preferred.to_lowercase()),
    }
}

fn effective_session_settings(state: &Value, session_id: &str) -> Value {
    let mut settings = state["settings"].clone();
    let session = state["sessions"]
        .as_array()
        .and_then(|sessions| sessions.iter().find(|session| session["id"] == session_id));
    let project = session
        .and_then(|session| session["projectId"].as_str())
        .and_then(|project_id| {
            state["projects"]
                .as_array()
                .and_then(|projects| projects.iter().find(|project| project["id"] == project_id))
        });
    let mut entries = Vec::<String>::new();
    let mut seen = HashSet::new();
    let mut bytes = 0usize;
    for source in [
        session.and_then(|session| session["customVocabulary"].as_array()),
        project.and_then(|project| project["customVocabulary"].as_array()),
        state["settings"]["customVocabulary"].as_array(),
    ] {
        for entry in source.into_iter().flatten().filter_map(Value::as_str) {
            let trimmed = entry.trim();
            if trimmed.is_empty() || !seen.insert(vocabulary_identity(trimmed)) {
                continue;
            }
            if entries.len() == 200 || bytes.saturating_add(trimmed.len()) > 10_000 {
                break;
            }
            bytes = bytes.saturating_add(trimmed.len());
            entries.push(trimmed.to_owned());
        }
    }
    settings["customVocabulary"] = json!(entries);
    settings
}
fn vocabulary_prompt(settings: &Value) -> String {
    let terms = vocabulary_terms(settings);
    if terms.is_empty() {
        String::new()
    } else {
        format!("Vocabulary: {}.", terms.join(", "))
    }
}

fn sanitize_vocabulary_prompt_leak(text: &str, settings: &Value) -> String {
    let mut fragments =
        vec!["Transcribe the audio faithfully. Apply these vocabulary spellings:".to_owned()];
    let current = vocabulary_prompt(settings);
    if !current.is_empty() {
        fragments.push(current);
    }
    for entry in settings["customVocabulary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|entry| !entry.trim().is_empty())
    {
        let (heard, preferred) = vocabulary_pair(entry);
        fragments.push(match heard {
            Some(heard) => format!("When the audio sounds like {heard:?}, write {preferred:?}."),
            None => format!("Use this exact spelling when spoken: {preferred:?}."),
        });
    }
    let mut cleaned = text.to_owned();
    let mut changed = false;
    for fragment in fragments {
        for candidate in [fragment.clone(), format!("- {fragment}")] {
            if cleaned.contains(&candidate) {
                cleaned = cleaned.replace(&candidate, "");
                changed = true;
            }
        }
    }
    if !changed {
        return text.trim().to_owned();
    }
    cleaned
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}
fn normalized_word(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}')
}
/// Whether a normalized word is a vocabulary stem or an inflection of it ("Tocquevillians"),
/// allowing the last two letters of the stem to change.
fn is_stem_form(word: &str, stem: &str) -> bool {
    let needed = stem.chars().count().saturating_sub(2).max(3);
    word.chars()
        .zip(stem.chars())
        .take_while(|(w, s)| w == s)
        .count()
        >= needed
}
/// Word stems of the preferred spellings; the bias also produces inflected forms of them.
fn vocabulary_stems(settings: &Value) -> Vec<String> {
    vocabulary_terms(settings)
        .iter()
        .flat_map(|term| term.split_whitespace().map(normalized_word))
        .filter(|stem| stem.chars().count() >= 3)
        .collect()
}
fn mentions_vocabulary(text: &str, settings: &Value) -> bool {
    let stems = vocabulary_stems(settings);
    text.split_whitespace().any(|word| {
        let word = normalized_word(word);
        stems.iter().any(|stem| is_stem_form(&word, stem))
    }) || vocabulary_terms(settings)
        .iter()
        .any(|term| term.chars().any(is_cjk) && text.contains(term.as_str()))
}
fn edit_distance(left: &[char], right: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, l) in left.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(l != r))
                .min(above + 1)
                .min(row[j] + 1);
            diagonal = above;
        }
    }
    row[right.len()]
}
/// Whether two spans plausibly came from the same sounds ("Tocqueville" / "Tokeville").
fn sounds_alike(left: &str, right: &str) -> bool {
    let (left, right): (Vec<char>, Vec<char>) = (left.chars().collect(), right.chars().collect());
    let longest = left.len().max(right.len());
    longest > 0 && edit_distance(&left, &right) * 2 <= longest
}
/// Keeps vocabulary words from the biased transcript only where the context-free transcript
/// heard something similar at the same place. Words the plain pass did not hear at all
/// (silence, trailing noise) are dropped; words it heard differently are replaced.
fn verify_vocabulary(biased: &str, plain: &str, settings: &Value) -> String {
    let stems = vocabulary_stems(settings);
    let is_term = |word: &str| {
        let word = normalized_word(word);
        !word.is_empty() && stems.iter().any(|stem| is_stem_form(&word, stem))
    };
    let has_words = |text: &str| text.chars().any(char::is_alphanumeric);
    if biased.chars().any(is_cjk) {
        // No word boundaries to align on; only reject output that is nothing but vocabulary.
        let mut rest = biased.to_owned();
        for term in vocabulary_terms(settings) {
            rest = rest.replace(term.as_str(), "");
        }
        let rest: String = rest
            .split_whitespace()
            .filter(|word| !is_term(word))
            .collect();
        return if has_words(&rest) {
            biased.to_owned()
        } else {
            plain.trim().to_owned()
        };
    }
    let heard: Vec<String> = settings["customVocabulary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|entry| vocabulary_pair(entry).0.map(normalized_word))
        .collect();
    let a: Vec<&str> = biased.split_whitespace().collect();
    let b: Vec<&str> = plain.split_whitespace().collect();
    let (na, nb): (Vec<String>, Vec<String>) = (
        a.iter().map(|w| normalized_word(w)).collect(),
        b.iter().map(|w| normalized_word(w)).collect(),
    );
    // Longest common subsequence table, filled from the end so the walk below runs forwards.
    let mut lcs = vec![vec![0u16; nb.len() + 1]; na.len() + 1];
    for i in (0..na.len()).rev() {
        for j in (0..nb.len()).rev() {
            lcs[i][j] = if na[i] == nb[j] && !na[i].is_empty() {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out: Vec<&str> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && na[i] == nb[j] && !na[i].is_empty() {
            out.push(a[i]);
            i += 1;
            j += 1;
            continue;
        }
        // Collect the differing block up to the next common word.
        let (start_a, start_b) = (i, j);
        while (i < a.len() || j < b.len())
            && !(i < a.len() && j < b.len() && na[i] == nb[j] && !na[i].is_empty())
        {
            if j >= b.len() || (i < a.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
                i += 1;
            } else {
                j += 1;
            }
        }
        let (block_a, block_b) = (&a[start_a..i], &b[start_b..j]);
        if !block_a.iter().any(|word| is_term(word)) {
            out.extend(block_a);
            continue;
        }
        let heard_b: String = nb[start_b..j].concat();
        if !block_b.is_empty()
            && (sounds_alike(&na[start_a..i].concat(), &heard_b) || heard.contains(&heard_b))
        {
            out.extend(block_a);
        } else if block_b.is_empty() {
            out.extend(block_a.iter().filter(|word| !is_term(word)));
        } else {
            out.extend(block_b);
        }
    }
    let text = out.join(" ");
    if has_words(&text) {
        text
    } else {
        String::new()
    }
}
fn same_model_settings(left: &Value, right: &Value) -> bool {
    [
        "engine",
        "executable",
        "modelPath",
        "mmprojPath",
        "language",
        "customVocabulary",
    ]
    .iter()
    .all(|key| left[*key] == right[*key])
}
const QWEN_SERVER_LOG: &str = "qwen-server.log";
const MODEL_CONNECTION_LOST: &str = "本地模型连接中断；已保存的音频可重试转写";
const SERVER_EXITED_EARLY: &str = "本地识别程序提前退出";
const MODEL_LOAD_TIMEOUT: &str = "本地模型加载超时，已保存的音频可稍后继续转写";
/// Written when the GPU backend failed on this computer. It names the app version, so a later
/// release (with a newer llama.cpp or driver workarounds) tries the GPU again once.
const QWEN_CPU_ONLY: &str = "qwen-cpu-only.txt";
fn gpu_allowed(root: &Path) -> bool {
    fs::read_to_string(root.join(QWEN_CPU_ONLY))
        .map(|marker| marker.lines().next() != Some(env!("CARGO_PKG_VERSION")))
        .unwrap_or(true)
}
fn remember_cpu_only(root: &Path, reason: &str) {
    let _ = fs::write(
        root.join(QWEN_CPU_ONLY),
        format!("{}\n{reason}\n", env!("CARGO_PKG_VERSION")),
    );
}
/// Names the exit code and the last error llama-server printed, so a user's screenshot is diagnosable.
fn early_exit_message(code: Option<i32>, log_path: &Path) -> String {
    let log = fs::read_to_string(log_path).unwrap_or_default();
    let mut message = SERVER_EXITED_EARLY.to_owned();
    if let Some(code) = code {
        message.push_str(&format!("（代码 0x{:08X}）", code as u32));
    }
    let detail = log
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("warning:"))
        .map(|line| line.chars().take(200).collect::<String>());
    match detail {
        Some(line) => message.push_str(&format!("：{line}")),
        None => message.push_str("，请检查模型与程序是否匹配"),
    }
    message.push_str(&format!("。详细日志：{}", log_path.display()));
    message
}
impl Drop for Model {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Model {
    fn server_alive(&self) -> bool {
        if self.settings["engine"] == "whisper" {
            return true;
        }
        self.child
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }
    /// Starts llama-server on the GPU unless this computer is known to need the CPU. A GPU start
    /// that exits or never answers is retried once on the CPU, and the CPU is then remembered.
    fn load(
        settings: Value,
        root: &Path,
        child: Arc<Mutex<Option<Child>>>,
        exit: &AtomicBool,
    ) -> Result<Self, String> {
        if settings["engine"] == "whisper" || !gpu_allowed(root) {
            return Self::load_on(settings, root, child, exit, false).map_err(|(error, _)| error);
        }
        match Self::load_on(settings.clone(), root, child.clone(), exit, true) {
            Ok(model) => Ok(model),
            Err((_, false)) | Err((_, true)) if exit.load(Ordering::SeqCst) => Err("应用正在退出".into()),
            Err((error, false)) => Err(error),
            Err((gpu_error, true)) => {
                if let Some(mut stale) = child.lock().unwrap().take() {
                    let _ = stale.kill();
                    let _ = stale.wait();
                }
                remember_cpu_only(root, &gpu_error);
                Self::load_on(settings, root, child, exit, false).map_err(|(error, _)| error)
            }
        }
    }
    /// The error's flag says whether llama-server itself failed after starting, which is the
    /// case a different device can fix; missing files and settings are not.
    fn load_on(
        settings: Value,
        root: &Path,
        child: Arc<Mutex<Option<Child>>>,
        exit: &AtomicBool,
        gpu: bool,
    ) -> Result<Self, (String, bool)> {
        Self::start(settings, root, child, exit, gpu).map_err(|error| {
            let server_failed =
                error.starts_with(SERVER_EXITED_EARLY) || error == MODEL_LOAD_TIMEOUT;
            (error, server_failed)
        })
    }
    fn start(
        settings: Value,
        root: &Path,
        child: Arc<Mutex<Option<Child>>>,
        exit: &AtomicBool,
        gpu: bool,
    ) -> Result<Self, String> {
        if exit.load(Ordering::SeqCst) {
            return Err("应用正在退出".into());
        }
        let exe = field(&settings, "executable")?;
        let model = field(&settings, "modelPath")?;
        if !Path::new(&exe).is_file() || !Path::new(&model).is_file() {
            return Err("请在设置中选择本地识别程序和模型文件；录音已继续保存在本机".into());
        }
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;
        let mut loaded = Self {
            settings,
            child,
            port: 0,
            key: uid(),
            client,
            gpu,
        };
        if loaded.settings["engine"] == "whisper" {
            return Ok(loaded);
        }
        let mmproj = field(&loaded.settings, "mmprojPath")?;
        if !Path::new(&mmproj).is_file() {
            return Err("请选择与 Qwen 模型配套的音频投影文件".into());
        }
        let socket = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        loaded.port = socket.local_addr().map_err(|e| e.to_string())?.port();
        drop(socket);
        let mut process = crate::native_process::command(exe);
        process
            .args([
                "-m",
                &model,
                "--mmproj",
                &mmproj,
                "--host",
                "127.0.0.1",
                "--port",
                &loaded.port.to_string(),
            ])
            .args([
                "--no-webui",
                "--jinja",
            ])
            .args(crate::native_process::qwen_device_args(gpu))
            .args([
                "-c",
                "4096",
                "-np",
                "1",
                // Each ASR request is a fresh audio prompt, so the prompt cache never hits; its
                // default 8 GiB budget only copies KV state and grows memory on small laptops.
                "--cache-ram",
                "0",
            ])
            // The key travels in the environment: llama.cpp opens --api-key-file with the ANSI code page
            // on Windows, which fails under non-ASCII user profile paths.
            .env("LLAMA_API_KEY", &loaded.key)
            .stdout(Stdio::null())
            .stderr(
                File::create(root.join(QWEN_SERVER_LOG))
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
            );
        *loaded.child.lock().unwrap() = Some(
            process
                .spawn_tied()
                .map_err(|e| format!("无法启动本地模型：{e}"))?,
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(90) {
            if exit.load(Ordering::SeqCst) {
                return Err("应用正在退出".into());
            }
            if let Some(status) = loaded
                .child
                .lock()
                .unwrap()
                .as_mut()
                .ok_or("模型已停止")?
                .try_wait()
                .map_err(|e| e.to_string())?
            {
                return Err(early_exit_message(
                    status.code(),
                    &root.join(QWEN_SERVER_LOG),
                ));
            }
            if loaded
                .client
                .get(format!("http://127.0.0.1:{}/health", loaded.port))
                .bearer_auth(&loaded.key)
                .timeout(Duration::from_secs(2))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                return Ok(loaded);
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(MODEL_LOAD_TIMEOUT.into())
    }
    fn transcribe(
        &mut self,
        wav: &[u8],
        root: &Path,
        request_settings: &Value,
    ) -> Result<String, String> {
        if self.settings["engine"] == "whisper" {
            let wav_path = root.join(format!("infer-{}.wav", uid()));
            atomic_write(&wav_path, wav)?;
            let prefix = wav_path.with_extension("output");
            let child = crate::native_process::command(field(&self.settings, "executable")?)
                .arg("-m")
                .arg(field(&self.settings, "modelPath")?)
                .arg("-f")
                .arg(&wav_path)
                .args(["-nt", "-np", "-otxt", "-of"])
                .arg(&prefix)
                .arg("-l")
                .arg(self.settings["language"].as_str().unwrap_or("auto"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn_tied()
                .map_err(|e| e.to_string())?;
            *self.child.lock().unwrap() = Some(child);
            let start = Instant::now();
            let result = loop {
                let status = self
                    .child
                    .lock()
                    .unwrap()
                    .as_mut()
                    .ok_or("模型已停止")?
                    .try_wait()
                    .map_err(|e| e.to_string())?;
                if let Some(status) = status {
                    break if status.success() {
                        fs::read_to_string(prefix.with_extension("output.txt"))
                            .map_err(|e| e.to_string())
                    } else {
                        Err("Whisper 转写失败".into())
                    };
                }
                if start.elapsed() > Duration::from_secs(120) {
                    if let Some(mut child) = self.child.lock().unwrap().take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    break Err("Whisper 转写超时".into());
                }
                thread::sleep(Duration::from_millis(50));
            };
            let _ = fs::remove_file(&wav_path);
            let _ = fs::remove_file(prefix.with_extension("output.txt"));
            return result.map(|t| t.trim().to_owned());
        }
        let context = vocabulary_prompt(request_settings);
        let biased = sanitize_vocabulary_prompt_leak(&self.qwen(wav, &context)?, request_settings);
        if context.is_empty() || !mentions_vocabulary(&biased, request_settings) {
            return Ok(biased);
        }
        // The vocabulary context biases Qwen strongly enough that it sometimes emits a term for
        // silence or swaps it in for a different word. A context-free pass vouches for each term.
        let plain = self.qwen(wav, "")?;
        Ok(verify_vocabulary(&biased, &plain, request_settings))
    }
    fn qwen(&self, wav: &[u8], context: &str) -> Result<String, String> {
        let mut messages = Vec::new();
        if !context.is_empty() {
            messages.push(json!({"role":"system","content":context}));
        }
        messages.push(json!({"role":"user","content":[{"type":"input_audio","input_audio":{"data":STANDARD.encode(wav),"format":"wav"}}]}));
        // DRY penalises extending a sequence the output already repeated, which breaks a greedy
        // "as, as, as, …" loop within a few words; ordinary transcripts, including spoken repeats
        // such as "这个这个", came out unchanged in English and Chinese tests.
        let mut payload = json!({"model":self.settings["modelPath"],"messages":messages,"stream":false,"temperature":0,"max_tokens":qwen_token_budget(wav.len()),"dry_multiplier":0.8,"dry_base":1.75,"dry_allowed_length":2,"dry_penalty_last_n":256});
        // Qwen's official forced-language protocol appends this assistant prefix.
        let language = match self.settings["language"].as_str() {
            Some("en") => Some("English"),
            Some("zh") => Some("Chinese"),
            _ => None,
        };
        if let Some(language) = language {
            payload["messages"].as_array_mut().unwrap().push(
                json!({"role":"assistant","content":format!("language {language}<asr_text>")}),
            );
            payload["continue_final_message"] = json!(true);
            payload["add_generation_prompt"] = json!(false);
        }
        let mut response = self
            .client
            .post(format!(
                "http://127.0.0.1:{}/v1/chat/completions",
                self.port
            ))
            .bearer_auth(&self.key)
            .json(&payload)
            .send()
            .map_err(|_| MODEL_CONNECTION_LOST)?;
        if !response.status().is_success() {
            return Err(format!("本地模型返回 {}，音频已保留", response.status()));
        }
        let mut bytes = Vec::new();
        Read::take(&mut response, 1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 1_048_576 {
            return Err("识别响应超出限制".into());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let raw = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or("本地模型响应格式无效")?;
        Ok(collapse_repetition(&parse_qwen(raw)))
    }
}
/// Speech runs at most ~6 tokens a second in English or Chinese; twice that plus the language tag
/// leaves room for fast speakers. Greedy decoding can fall into a loop ("as, as, as, …"); an
/// open-ended budget let one loop run for 1024 tokens, which on a CPU took long enough to time
/// out and stall every segment queued behind it.
fn qwen_token_budget(wav_bytes: usize) -> u64 {
    let seconds = wav_bytes.saturating_sub(44) as f64 / 32_000.0;
    ((seconds * 12.0).ceil() as u64 + 32).clamp(48, 256)
}
/// A unit of one to eight words (or CJK characters) repeated four or more times in a row is a
/// decoding loop, not speech; it is kept once. Three repeats ("very, very, very") are left alone.
fn collapse_repetition(text: &str) -> String {
    const MIN_REPEATS: usize = 4;
    const MAX_UNIT: usize = 8;
    let cjk = |c: char| ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c);
    let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
    let by_char = letters > 0 && text.chars().filter(|c| cjk(*c)).count() * 2 > letters;
    let tokens: Vec<String> = if by_char {
        text.chars().filter(|c| !c.is_whitespace()).map(String::from).collect()
    } else {
        text.split_whitespace().map(str::to_owned).collect()
    };
    let key = |token: &String| -> String {
        token.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
    };
    let keys: Vec<String> = tokens.iter().map(key).collect();
    let mut kept: Vec<&str> = Vec::with_capacity(tokens.len());
    let mut collapsed = false;
    let mut i = 0;
    while i < tokens.len() {
        let mut skipped = false;
        for unit in 1..=MAX_UNIT {
            if i + unit * MIN_REPEATS > tokens.len() || keys[i..i + unit].iter().all(String::is_empty) {
                continue;
            }
            let mut repeats = 1;
            while i + (repeats + 1) * unit <= tokens.len()
                && keys[i + repeats * unit..i + (repeats + 1) * unit] == keys[i..i + unit]
            {
                repeats += 1;
            }
            if repeats >= MIN_REPEATS {
                kept.extend(tokens[i..i + unit].iter().map(String::as_str));
                i += repeats * unit;
                collapsed = true;
                skipped = true;
                break;
            }
        }
        if !skipped {
            kept.push(&tokens[i]);
            i += 1;
        }
    }
    if !collapsed {
        return text.to_owned();
    }
    let joined = kept.join(if by_char { "" } else { " " });
    joined.trim_end_matches([',', '，', '、', ' ']).to_owned()
}
fn parse_qwen(raw: &str) -> String {
    let text = raw.trim();
    match text.split_once("<asr_text>") {
        Some((prefix, body))
            if prefix.is_empty() || prefix.trim().to_lowercase().starts_with("language ") =>
        {
            body.trim().to_owned()
        }
        _ => text.to_owned(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FinalTextSegment {
    id: String,
    start: u64,
    end: u64,
    text: String,
}

fn split_final_text(job: &Job, text: &str) -> Vec<FinalTextSegment> {
    let sentences = natural_sentences(text);
    if sentences.len() <= 1 {
        return vec![FinalTextSegment {
            id: job.id.clone(),
            start: job.start,
            end: job.end,
            text: text.to_owned(),
        }];
    }
    let weights: Vec<u64> = sentences
        .iter()
        .map(|sentence| {
            sentence
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                .max(1) as u64
        })
        .collect();
    let total_weight: u64 = weights.iter().sum();
    let duration = job.end.saturating_sub(job.start);
    let mut cumulative_weight = 0u64;
    let mut start = job.start;
    sentences
        .into_iter()
        .enumerate()
        .map(|(index, sentence)| {
            cumulative_weight = cumulative_weight.saturating_add(weights[index]);
            let end = if index + 1 == weights.len() {
                job.end
            } else {
                job.start.saturating_add(
                    ((duration as u128 * cumulative_weight as u128) / total_weight as u128) as u64,
                )
            }
            .max(start)
            .min(job.end);
            let segment = FinalTextSegment {
                id: if index == 0 {
                    job.id.clone()
                } else {
                    format!("{}_sentence_{}", job.id, index + 1)
                },
                start,
                end,
                text: sentence,
            };
            start = end;
            segment
        })
        .collect()
}

/// Splitting would leave the human text on the first sentence as a conflict and duplicate the
/// rest as new machine segments, so an edited or open segment is finalised whole.
fn human_owned(session: &crate::domain::Session, segment_id: &str) -> bool {
    session
        .segments
        .iter()
        .any(|segment| segment.id == segment_id && !segment.history.is_empty())
        || session
            .drafts
            .iter()
            .any(|draft| draft.segment_id == segment_id && draft.state == "open")
}

fn natural_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0usize;
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let mut index = 0usize;
    while index < characters.len() {
        let (offset, character) = characters[index];
        let terminal = matches!(character, '.' | '!' | '?' | '。' | '！' | '？');
        let paragraph = character == '\n'
            && characters
                .get(index + 1)
                .is_some_and(|(_, next)| *next == '\n');
        if !terminal && !paragraph {
            index += 1;
            continue;
        }
        let mut boundary = offset + character.len_utf8();
        let mut next = index + 1;
        if terminal {
            while let Some((next_offset, next_character)) = characters.get(next).copied() {
                if !matches!(next_character, '.' | '!' | '?' | '。' | '！' | '？')
                    && !is_sentence_closer(next_character)
                {
                    break;
                }
                boundary = next_offset + next_character.len_utf8();
                next += 1;
            }
            // "3.14", "e.g. this": a period only ends a sentence before whitespace (or the end)
            // and a start that is not a digit or lowercase letter.
            if character == '.'
                && (characters
                    .get(next)
                    .is_some_and(|(_, next_character)| !next_character.is_whitespace())
                    || characters[next..]
                        .iter()
                        .find(|(_, next_character)| !next_character.is_whitespace())
                        .is_some_and(|(_, next_character)| {
                            next_character.is_ascii_digit() || next_character.is_lowercase()
                        }))
            {
                index += 1;
                continue;
            }
        }
        while let Some((next_offset, next_character)) = characters.get(next).copied() {
            if next_character.is_whitespace() {
                boundary = next_offset + next_character.len_utf8();
                next += 1;
                continue;
            }
            break;
        }
        let sentence = text[start..boundary].trim();
        if !sentence.is_empty() {
            sentences.push(sentence.to_owned());
        }
        start = boundary;
        index = next;
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        sentences.push(tail.to_owned());
    }
    if sentences.is_empty() {
        sentences.push(text.to_owned());
    }
    sentences
}

fn is_sentence_closer(character: char) -> bool {
    matches!(
        character,
        '"' | '\'' | '”' | '’' | '」' | '』' | ')' | ']' | '}' | '）' | '】' | '》'
    )
}

/// Called once no local job is pending; permanently failed jobs keep the session in error so a
/// later success cannot hide them.
fn settle_local_inference(session: &mut Value, failed: usize) {
    session["inferenceState"] = json!(if failed > 0 { "error" } else { "ready" });
    let gaps = session["gaps"].as_array().is_some_and(|g| !g.is_empty());
    if failed > 0 {
        if session["recordingState"] != "error" {
            session["error"] = json!(format!("有 {failed} 段音频转写失败，可重试"));
        }
    } else if session["recordingState"] != "error" && !gaps {
        session["error"] = Value::Null;
    } else if gaps {
        session["error"] = json!("检测到音频缺口，已保留时间位置和缺口记录");
    }
}
/// Seconds of model time per second of audio above which live previews are dropped. A preview
/// re-reads the whole growing segment every 2 s, so on a slow CPU previews alone outrun real time
/// and final text falls further and further behind the lecture.
const PREVIEW_PACE_LIMIT: f64 = 0.5;
fn worker_loop(weak: Weak<Runtime>) {
    let mut model: Option<Model> = None;
    // Smoothed model seconds per audio second; 0 until the first measurement.
    let mut pace = 0.0f64;
    let mut attempted_settings = Value::Null;
    loop {
        let Some(runtime) = weak.upgrade() else {
            break;
        };
        if runtime.exit.load(Ordering::Relaxed) {
            break;
        }
        let settings = runtime.value().unwrap_or_default()["settings"].clone();
        if runtime.prepare_requested.swap(false, Ordering::SeqCst)
            || !same_model_settings(&attempted_settings, &settings)
        {
            attempted_settings = settings.clone();
            model = None;
            if settings["engine"] == "whisper" {
                *runtime.model_status.lock().unwrap() = json!({
                    "state":"error",
                    "settings":settings,
                    "error":WHISPER_REALTIME_UNAVAILABLE
                });
            } else if settings["engine"] == "soniox" {
                *runtime.model_status.lock().unwrap() =
                    json!({"state":"unloaded","settings":settings});
            } else if model_files_present(&settings) {
                *runtime.model_status.lock().unwrap() =
                    json!({"state":"loading","settings":settings});
                match Model::load(
                    settings.clone(),
                    &runtime.root,
                    runtime.model_child.clone(),
                    &runtime.exit,
                ) {
                    Ok(loaded) => {
                        model = Some(loaded);
                        *runtime.model_status.lock().unwrap() =
                            json!({"state":"ready","settings":settings});
                    }
                    Err(error) => {
                        *runtime.model_status.lock().unwrap() =
                            json!({"state":"error","settings":settings,"error":error});
                    }
                }
            } else {
                *runtime.model_status.lock().unwrap() =
                    json!({"state":"unloaded","settings":settings});
            }
        }
        if settings["engine"] == "soniox" {
            drop(runtime);
            thread::sleep(Duration::from_millis(150));
            continue;
        }
        let jobs = match runtime.jobs() {
            Ok(j) => j,
            Err(_) => {
                drop(runtime);
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let mut job = if let Some(j) = jobs
            .into_iter()
            .find(|j| !j.failed && !runtime.job_is_cloud(j).unwrap_or(false))
        {
            j
        } else {
            let mut p = runtime.partial.lock().unwrap();
            if pace > PREVIEW_PACE_LIMIT {
                // Finals still arrive every few seconds; skipping previews keeps them on time.
                p.retain(|_, job| runtime.job_is_cloud(job).unwrap_or(false));
            }
            if let Some(key) = p
                .iter()
                .find(|(_, job)| !runtime.job_is_cloud(job).unwrap_or(false))
                .map(|(key, _)| key.clone())
            {
                p.remove(&key).unwrap()
            } else {
                drop(p);
                drop(runtime);
                thread::sleep(Duration::from_millis(150));
                continue;
            }
        };
        let result = (|| -> Result<(), String> {
            let session = runtime.session(&job.session)?;
            if runtime.job_is_cloud(&job)? {
                return Err("CLOUD_JOB_ON_LOCAL_WORKER".into());
            }
            if session["segments"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s["id"] == job.id && s["final"] == true))
            {
                return Ok(());
            }
            let request_state = runtime.value()?;
            let epoch = request_state["workerEpoch"].as_u64().unwrap_or(1);
            let settings = request_state["settings"].clone();
            let request_settings = effective_session_settings(&request_state, &job.session);
            if model
                .as_ref()
                .is_none_or(|m| !same_model_settings(&m.settings, &settings))
            {
                model = None;
                runtime.update(&job.session, |s| {
                    s["inferenceState"] = json!("loading");
                    Ok(())
                })?;
                model = Some(Model::load(
                    settings.clone(),
                    &runtime.root,
                    runtime.model_child.clone(),
                    &runtime.exit,
                )?);
                *runtime.model_status.lock().unwrap() = if settings["engine"] == "whisper" {
                    json!({"state":"error","settings":settings,"error":WHISPER_REALTIME_UNAVAILABLE})
                } else {
                    json!({"state":"ready","settings":settings})
                };
            }
            runtime.update(&job.session, |s| {
                s["inferenceState"] = json!(if s["recordingState"] == "recording" {
                    "running"
                } else {
                    "catching_up"
                });
                Ok(())
            })?;
            if runtime.exit.load(Ordering::Relaxed) {
                return Err("转写将在下次打开时继续".into());
            }
            let wav = runtime.wav(&job)?;
            let text = if has_speech(&wav[44..]) {
                let started = Instant::now();
                let text = model
                    .as_mut()
                    .unwrap()
                    .transcribe(&wav, &runtime.root, &request_settings)?;
                let seconds = (wav.len() - 44) as f64 / 32_000.0;
                if seconds >= 1.0 {
                    let rate = started.elapsed().as_secs_f64() / seconds;
                    pace = if pace == 0.0 { rate } else { pace * 0.7 + rate * 0.3 };
                }
                text
            } else {
                String::new()
            };
            let mut store = runtime.store.lock().map_err(|_| "数据写入器不可用")?;
            let current = store.snapshot();
            if current.worker_epoch != epoch {
                return Err("STALE_WORKER_EPOCH".into());
            }
            let session = current
                .sessions
                .iter()
                .find(|session| session.id == job.session)
                .ok_or("课程不存在")?;
            let revision = session
                .segments
                .iter()
                .find(|segment| segment.id == job.id)
                .map_or(0, |segment| segment.machine_revision)
                + 1;
            let final_segments = if human_owned(session, &job.id) {
                Vec::new()
            } else {
                split_final_text(&job, &text)
            };
            if job.final_result && final_segments.len() > 1 {
                let segments: Vec<Value> = final_segments
                    .iter()
                    .enumerate()
                    .map(|(index, segment)| {
                        json!({
                            "id": segment.id,
                            "startSample": segment.start,
                            "endSample": segment.end,
                            "text": segment.text,
                            "revision": if index == 0 { revision } else { 1 }
                        })
                    })
                    .collect();
                store.apply(json!({"type":"machineSegments","commandId":uid(),"sessionId":job.session,"runId":job.run,"segments":segments,"workerEpoch":epoch}))?;
                drop(store);
                for segment in &final_segments {
                    runtime.schedule_auto_polish(&job.session, &segment.id);
                }
            } else {
                store.apply(json!({"type":"machine","commandId":uid(),"sessionId":job.session,"segmentId":job.id,"runId":job.run,"startSample":job.start,"endSample":job.end,"text":text,"revision":revision,"workerEpoch":epoch,"final":job.final_result}))?;
                drop(store);
                if job.final_result {
                    runtime.schedule_auto_polish(&job.session, &job.id);
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                if job.final_result {
                    let _ = fs::remove_file(runtime.job_path(&job));
                }
                let local: Vec<Job> = runtime
                    .jobs()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|j| {
                        j.session == job.session && !runtime.job_is_cloud(j).unwrap_or(false)
                    })
                    .collect();
                let failed = local.iter().filter(|j| j.failed).count();
                if failed == local.len() {
                    let _ = runtime.update(&job.session, |s| {
                        settle_local_inference(s, failed);
                        Ok(())
                    });
                }
            }
            Err(e) => {
                // A job-level failure (deleted session, unreadable WAV, HTTP 5xx) leaves a healthy
                // server; reloading ~1 GB of weights for it only stalls the queue.
                if !model.as_ref().is_some_and(Model::server_alive) || e == MODEL_CONNECTION_LOST {
                    // A GPU driver that crashes llama-server mid-lecture would crash it again on
                    // every reload; the next load goes straight to the CPU instead.
                    if model.as_ref().is_some_and(|m| m.gpu && !m.server_alive()) {
                        remember_cpu_only(&runtime.root, &format!("stopped during transcription: {e}"));
                    }
                    model = None;
                    *runtime.model_status.lock().unwrap() =
                        json!({"state":"error","settings":settings,"error":e});
                }
                if e == "STALE_WORKER_EPOCH" {
                    if !job.final_result {
                        let _ = runtime.enqueue(job);
                    }
                    continue;
                }
                if runtime.exit.load(Ordering::Relaxed) {
                    break;
                }
                job.attempts += 1;
                if job.final_result {
                    job.failed = job.attempts >= 3;
                    let _ = runtime.save_job(&job);
                }
                runtime.error(&job.session, &e, true);
                drop(runtime);
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        }
        drop(runtime);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn early_exit_names_code_and_last_server_error() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join(QWEN_SERVER_LOG);
        fs::write(&log, "warning: no usable GPU found\nload_model: failed to load mmproj\n\nwarning: consult docs\n").unwrap();
        let message = early_exit_message(Some(-1073740791), &log);
        assert!(message.starts_with(
            "本地识别程序提前退出（代码 0xC0000409）：load_model: failed to load mmproj"
        ));
        assert!(message.ends_with(&log.display().to_string()));
        let missing = early_exit_message(None, &temp.path().join("absent.log"));
        assert!(missing.contains("请检查模型与程序是否匹配"));
    }
    /// Plays a 16 kHz mono WAV through recording, segmenting and the local model at real-time
    /// speed and reports how long each final sentence took to appear after it was spoken.
    /// QWEN_SERVER, QWEN_MODEL, QWEN_MMPROJ and LAG_WAV name the files; run with --ignored --nocapture.
    #[test]
    #[ignore = "needs the local Qwen model and a recording"]
    fn live_transcript_lag_on_the_real_model() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
        let wav = fs::read(var("LAG_WAV")).unwrap();
        let data = wav.windows(4).position(|w| w == b"data").expect("WAV data chunk") + 8;
        let samples: Vec<f32> = wav[data..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0)
            .collect();
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        runtime
            .store
            .lock()
            .unwrap()
            .mutate(|state| {
                state.settings.engine = "qwen".into();
                state.settings.executable = var("QWEN_SERVER");
                state.settings.model_path = var("QWEN_MODEL");
                state.settings.mmproj_path = var("QWEN_MMPROJ");
                state.settings.language = "en".into();
                Ok(())
            })
            .unwrap();
        let sid = runtime
            .dispatch(json!({"type":"createSession","commandId":uid(),"title":"lag","mode":"live"}))
            .unwrap()
            .selected_session_id
            .unwrap();
        let rid = runtime.add_run(&sid, "microphone").unwrap();
        let weak = Arc::downgrade(&runtime);
        let worker = thread::spawn(move || worker_loop(weak));
        // Let the model load before the lecture starts, as it does when a user presses record.
        let loaded = Instant::now();
        while runtime.model_status.lock().unwrap()["state"] != "ready" && loaded.elapsed() < Duration::from_secs(120) {
            let _ = runtime.enqueue(Job { id: "warm".into(), session: sid.clone(), run: rid.clone(), start: 0, end: 0, final_result: false, attempts: 0, failed: false });
            thread::sleep(Duration::from_millis(200));
        }
        let mut archiver = Archiver::new(runtime.clone(), sid.clone(), rid.clone(), 16000).unwrap();
        let started = Instant::now();
        let mut seen: BTreeMap<String, f64> = BTreeMap::new();
        let mut lags: Vec<f64> = Vec::new();
        let mut previews = 0usize;
        let mut last_preview_text = String::new();
        let mut observe = |runtime: &Arc<Runtime>, lags: &mut Vec<f64>, previews: &mut usize| {
            let now = started.elapsed().as_secs_f64();
            let session = runtime.session(&sid).unwrap();
            for segment in session["segments"].as_array().into_iter().flatten() {
                let id = segment["id"].as_str().unwrap_or_default().to_owned();
                if segment["final"] == true && !seen.contains_key(&id) {
                    let spoken = segment["endSample"].as_u64().unwrap_or(0) as f64 / 16000.0;
                    seen.insert(id, now);
                    lags.push(now - spoken);
                } else if segment["final"] != true {
                    let text = segment["machineText"].as_str().unwrap_or_default().to_owned();
                    if text != last_preview_text {
                        last_preview_text = text;
                        *previews += 1;
                    }
                }
            }
        };
        for (index, chunk) in samples.chunks(1600).enumerate() {
            let due = Duration::from_millis(index as u64 * 100);
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                thread::sleep(wait);
            }
            archiver.frame(capture::Frame { start: (index * 1600) as u64, samples: chunk.to_vec() }).unwrap();
            if index % 2 == 0 {
                observe(&runtime, &mut lags, &mut previews);
            }
        }
        archiver.finish().unwrap();
        let audio_end = started.elapsed().as_secs_f64();
        while runtime.jobs().unwrap().iter().any(|job| !job.failed) && started.elapsed().as_secs_f64() < audio_end + 600.0 {
            observe(&runtime, &mut lags, &mut previews);
            thread::sleep(Duration::from_millis(100));
        }
        thread::sleep(Duration::from_millis(300));
        observe(&runtime, &mut lags, &mut previews);
        let done = started.elapsed().as_secs_f64();
        runtime.exit.store(true, Ordering::SeqCst);
        drop(runtime.model_child.lock().unwrap().take().map(|mut child| child.kill()));
        let session = runtime.session(&sid).unwrap();
        let text: Vec<String> = session["segments"].as_array().into_iter().flatten().map(|s| s["machineText"].as_str().unwrap_or_default().to_owned()).collect();
        let mut sorted = lags.clone();
        sorted.sort_by(f64::total_cmp);
        let pick = |q: f64| sorted.get(((sorted.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
        println!("LAG {}", json!({
            "audio_seconds": samples.len() as f64 / 16000.0,
            "finals": lags.len(),
            "preview_updates": previews,
            "lag_p50": pick(0.5), "lag_p90": pick(0.9), "lag_max": pick(1.0),
            "drain_after_audio_end": done - audio_end,
            "errors": session["error"],
        }));
        println!("TEXT {}", text.join(" "));
        drop(worker);
    }
    /// A llama-server that crashes whenever it may use the GPU, like a broken Vulkan driver.
    #[cfg(unix)]
    #[test]
    fn a_gpu_that_fails_to_start_falls_back_to_the_cpu_and_is_remembered() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let server = temp.path().join("llama-server");
        fs::write(&server, r#"#!/bin/sh
echo "$*" >> "$(dirname "$0")/calls.log"
case "$*" in *"-ngl 99"*) echo "ggml_vulkan: vk::Device::createComputePipeline: ErrorDeviceLost" >&2; exit 5;; esac
while [ "$1" != "--port" ]; do shift; done
exec python3 -c 'import http.server,sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self): self.send_response(200); self.end_headers(); self.wfile.write(b"{}")
    def log_message(self,*a): pass
http.server.HTTPServer(("127.0.0.1",int(sys.argv[1])),H).serve_forever()' "$2"
"#).unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        for name in ["model.gguf", "mmproj.gguf"] {
            fs::write(temp.path().join(name), b"stub").unwrap();
        }
        let settings = json!({"engine":"qwen","executable":server,"modelPath":temp.path().join("model.gguf"),"mmprojPath":temp.path().join("mmproj.gguf"),"language":"en"});
        let exit = AtomicBool::new(false);
        let child = Arc::new(Mutex::new(None));
        let model = Model::load(settings.clone(), temp.path(), child.clone(), &exit).unwrap();
        assert!(!model.gpu);
        assert!(!gpu_allowed(temp.path()));
        let marker = fs::read_to_string(temp.path().join(QWEN_CPU_ONLY)).unwrap();
        assert!(marker.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(marker.contains("本地识别程序提前退出"));
        drop(model);
        let again = Model::load(settings, temp.path(), child, &exit).unwrap();
        assert!(!again.gpu);
        drop(again);
        let calls = fs::read_to_string(temp.path().join("calls.log")).unwrap();
        let calls: Vec<&str> = calls.lines().collect();
        assert_eq!(calls.len(), 3, "GPU try, CPU retry, then CPU directly: {calls:?}");
        assert!(calls[0].contains("-ngl 99"));
        assert!(calls[1].contains("-ngl 0 -dev none --no-mmproj-offload"));
        assert!(calls[2].contains("-ngl 0 -dev none --no-mmproj-offload"));
        // A newer release tries the GPU again.
        fs::write(temp.path().join(QWEN_CPU_ONLY), "0.0.1\nold\n").unwrap();
        assert!(gpu_allowed(temp.path()));
    }
    use crate::soniox::Update;
    fn isolated_runtime(root: &Path) -> Arc<Runtime> {
        fs::create_dir_all(root.join("jobs")).unwrap();
        Arc::new(Runtime {
            store: Arc::new(Mutex::new(Store::open(&root.join("test.sqlite")).unwrap())),
            root: root.to_owned(),
            resources: root.join("resources"),
            capture: Mutex::new(None),
            partial: Mutex::new(BTreeMap::new()),
            native_receipts: Mutex::new(()),
            exit: AtomicBool::new(false),
            worker: Mutex::new(None),
            model_child: Arc::new(Mutex::new(None)),
            model_status: Mutex::new(json!({"state":"unloaded"})),
            model_download: Mutex::new(json!({
                "phase":"idle",
                "downloadedBytes":0,
                "totalBytes":crate::models::install_total_bytes(),
                "fileName":Value::Null,
                "error":Value::Null
            })),
            model_installing: AtomicBool::new(false),
            local_model_verification: Arc::new(Mutex::new(Vec::new())),
            prepare_requested: AtomicBool::new(false),
            persist_credentials: false,
            soniox_key: Mutex::new(None),
            deepseek_credential: Arc::new(Mutex::new(DeepSeekCredential::default())),
            auto_polish_scheduled: Arc::new(Mutex::new(HashSet::new())),
            auto_polish_status: Arc::new(Mutex::new(json!({"state":"idle"}))),
            auto_polish_generation: Arc::new(Mutex::new(HashMap::new())),
            cloud: Mutex::new(None),
        })
    }
    fn session(runtime: &Arc<Runtime>, title: &str) -> String {
        runtime
            .dispatch(json!({"type":"createSession","commandId":uid(),"title":title,"mode":"live"}))
            .unwrap()
            .selected_session_id
            .unwrap()
    }
    fn select_soniox(runtime: &Arc<Runtime>, consent: bool) {
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.engine = "soniox".into();
        settings.cloud_consent = consent;
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();
    }
    struct FakeCloud {
        sent: usize,
        finishing: bool,
        finished: bool,
        provisional_sent: bool,
    }
    impl CloudStream for FakeCloud {
        fn send_audio(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.sent += bytes.len();
            Ok(())
        }
        fn finish_input(&mut self) -> Result<(), String> {
            self.finishing = true;
            Ok(())
        }
        fn poll(&mut self) -> Result<Vec<Update>, String> {
            if self.finishing && !self.finished {
                self.finished = true;
                return Ok(vec![Update {
                    index: 0,
                    start_sample: 0,
                    end_sample: 10_000,
                    text: "final".into(),
                    final_result: true,
                }]);
            }
            if self.sent > 0 && !self.provisional_sent {
                self.provisional_sent = true;
                return Ok(vec![Update {
                    index: 0,
                    start_sample: 16,
                    end_sample: 20_000,
                    text: "partial".into(),
                    final_result: false,
                }]);
            }
            Ok(Vec::new())
        }
        fn finished(&self) -> bool {
            self.finished
        }
    }
    #[test]
    fn soniox_readiness_requires_consent_and_keeps_key_out_of_course_data() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "memory key");
        select_soniox(&runtime, false);
        assert_eq!(runtime.info()["modelInstalled"], true);
        assert_eq!(runtime.info()["modelReady"], false);
        assert!(runtime
            .soniox_configuration(&sid)
            .unwrap_err()
            .contains("同意"));
        let secret = "soniox-test-secret-48291";
        runtime
            .dispatch(json!({"type":"configureSoniox","apiKey":secret}))
            .unwrap();
        assert_eq!(runtime.info()["cloudKeyConfigured"], true);
        assert_eq!(runtime.info()["modelReady"], false);
        select_soniox(&runtime, true);
        assert_eq!(runtime.info()["modelState"], "ready");
        let snapshot = serde_json::to_string(&runtime.snapshot().unwrap()).unwrap();
        assert!(!snapshot.contains(secret));
        let exported = crate::archive::export_markdown(&runtime.snapshot().unwrap().sessions[0]);
        assert!(!exported.contains(secret));
        for entry in fs::read_dir(temp.path()).unwrap().flatten() {
            if entry.file_name().to_string_lossy().contains("sqlite") {
                assert!(!fs::read(entry.path())
                    .unwrap()
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()));
            }
        }
    }
    #[test]
    fn pause_and_stop_use_distinct_recording_states() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "pause state");
        runtime
            .update(&sid, |session| {
                session["recordingState"] = json!("recording");
                Ok(())
            })
            .unwrap();
        runtime
            .dispatch(json!({"type":"pauseRecording","commandId":uid(),"sessionId":sid.clone()}))
            .unwrap();
        assert_eq!(runtime.session(&sid).unwrap()["recordingState"], "paused");
        runtime
            .dispatch(json!({"type":"stopRecording","commandId":uid(),"sessionId":sid}))
            .unwrap();
        assert_eq!(runtime.session(&sid).unwrap()["recordingState"], "stopped");
    }
    #[test]
    fn deepseek_key_stays_out_of_course_data_and_can_be_cleared() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        session(&runtime, "deepseek memory key");
        let secret = "deepseek-test-secret-72841";
        let initial_epoch = runtime.deepseek_credential.lock().unwrap().epoch;
        runtime
            .dispatch(json!({"type":"configureDeepSeek","apiKey":secret}))
            .unwrap();
        assert_eq!(
            runtime.deepseek_credential.lock().unwrap().epoch,
            initial_epoch + 1
        );
        assert_eq!(runtime.info()["deepseekKeyConfigured"], true);
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.auto_polish = true;
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();
        assert_eq!(
            runtime.deepseek_credential.lock().unwrap().epoch,
            initial_epoch + 2
        );
        assert!(runtime.snapshot().unwrap().settings.auto_polish);
        let snapshot = serde_json::to_string(&runtime.snapshot().unwrap()).unwrap();
        assert!(!snapshot.contains(secret));
        let exported = crate::archive::export_markdown(&runtime.snapshot().unwrap().sessions[0]);
        assert!(!exported.contains(secret));
        for entry in fs::read_dir(temp.path()).unwrap().flatten() {
            if entry.file_name().to_string_lossy().contains("sqlite") {
                assert!(!fs::read(entry.path())
                    .unwrap()
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()));
            }
        }
        runtime
            .dispatch(json!({"type":"configureDeepSeek","apiKey":""}))
            .unwrap();
        assert_eq!(
            runtime.deepseek_credential.lock().unwrap().epoch,
            initial_epoch + 3
        );
        assert_eq!(runtime.info()["deepseekKeyConfigured"], false);
        assert!(!runtime.snapshot().unwrap().settings.auto_polish);
        assert_eq!(
            runtime.deepseek_api_key().unwrap_err(),
            "请先配置 DeepSeek API 密钥"
        );
    }

    #[test]
    fn auto_polish_requires_a_configured_deepseek_key() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.auto_polish = true;
        assert_eq!(
            runtime
                .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
                .unwrap_err(),
            "请先配置 DeepSeek API Key，再启用转写轻度整理"
        );
        runtime
            .dispatch(json!({"type":"configureDeepSeek","apiKey":"memory-only-key"}))
            .unwrap();
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.auto_polish = true;
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();
        assert!(runtime.snapshot().unwrap().settings.auto_polish);
    }
    #[test]
    fn clearing_the_memory_key_cancels_an_active_cloud_task() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        runtime
            .dispatch(json!({"type":"configureSoniox","apiKey":"memory-only"}))
            .unwrap();
        let active_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = active_stop.clone();
        let active_thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        *runtime.cloud.lock().unwrap() = Some(CloudTask {
            session: "active-session".into(),
            stop: active_stop.clone(),
            thread: active_thread,
        });

        runtime
            .dispatch(json!({"type":"configureSoniox","apiKey":""}))
            .unwrap();
        assert!(active_stop.load(Ordering::SeqCst));
        assert_eq!(runtime.info()["cloudKeyConfigured"], false);
        let task = runtime.cloud.lock().unwrap().take().unwrap();
        task.thread.join().unwrap();
    }
    #[test]
    fn run_engine_pins_worker_routing() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "routing");
        select_soniox(&runtime, true);
        let cloud_run_id = runtime.add_run(&sid, "import").unwrap();
        let cloud_job = Job {
            id: uid(),
            session: sid.clone(),
            run: cloud_run_id,
            start: 0,
            end: 160,
            final_result: true,
            attempts: 0,
            failed: false,
        };
        assert!(runtime.job_is_cloud(&cloud_job).unwrap());
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.engine = "qwen".into();
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();
        let local_run_id = runtime.add_run(&sid, "import").unwrap();
        let local_job = Job {
            run: local_run_id,
            ..cloud_job
        };
        assert!(!runtime.job_is_cloud(&local_job).unwrap());
    }
    #[test]
    fn retry_rejects_an_active_cloud_task_without_changing_state() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "retry target");
        select_soniox(&runtime, true);
        runtime
            .dispatch(json!({"type":"configureSoniox","apiKey":"memory-only"}))
            .unwrap();
        let rid = runtime.add_run(&sid, "import").unwrap();
        let job = Job {
            id: format!("{rid}_0"),
            session: sid.clone(),
            run: rid,
            start: 0,
            end: 160,
            final_result: true,
            attempts: 2,
            failed: true,
        };
        runtime.save_job(&job).unwrap();
        runtime
            .update(&sid, |session| {
                session["inferenceState"] = json!("error");
                session["error"] = json!("existing failure");
                Ok(())
            })
            .unwrap();

        let active_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = active_stop.clone();
        let active_thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        *runtime.cloud.lock().unwrap() = Some(CloudTask {
            session: "different-session".into(),
            stop: active_stop.clone(),
            thread: active_thread,
        });

        let epoch_before = runtime.snapshot().unwrap().worker_epoch;
        let session_before = runtime.session(&sid).unwrap();
        let job_before = fs::read(runtime.job_path(&job)).unwrap();
        let result =
            runtime.dispatch(json!({"type":"retryInference","commandId":uid(),"sessionId":sid}));
        let epoch_after = runtime.snapshot().unwrap().worker_epoch;
        let session_after = runtime.session(&sid).unwrap();
        let job_after = fs::read(runtime.job_path(&job)).unwrap();
        let active_was_stopped = active_stop.load(Ordering::SeqCst);
        let active_was_finished = runtime
            .cloud
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .thread
            .is_finished();
        let task = runtime.cloud.lock().unwrap().take().unwrap();
        task.stop.store(true, Ordering::SeqCst);
        task.thread.join().unwrap();

        assert!(result.unwrap_err().contains("另一项"));
        assert_eq!(epoch_after, epoch_before);
        assert_eq!(session_after, session_before);
        assert_eq!(job_after, job_before);
        assert!(!active_was_stopped);
        assert!(!active_was_finished);
    }
    #[test]
    fn fake_cloud_clamps_timestamps_and_keeps_partial_identity_and_start() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "cloud updates");
        select_soniox(&runtime, true);
        runtime
            .dispatch(json!({"type":"configureSoniox","apiKey":"memory-only"}))
            .unwrap();
        let rid = runtime.add_run(&sid, "import").unwrap();
        let path = runtime.run_path(&sid, &rid).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![0u8; 640]).unwrap();
        runtime
            .update(&sid, |value| {
                let run = value["runs"].as_array_mut().unwrap().last_mut().unwrap();
                run["samples"] = json!(320);
                run["state"] = json!("closed");
                Ok(())
            })
            .unwrap();
        let epoch = runtime.snapshot().unwrap().worker_epoch;
        let fake = FakeCloud {
            sent: 0,
            finishing: false,
            finished: false,
            provisional_sent: false,
        };
        cloud_run(
            &runtime,
            &sid,
            &rid,
            fake,
            CloudResume::default(),
            epoch,
            &AtomicBool::new(false),
        )
        .unwrap();
        let segment = &runtime.session(&sid).unwrap()["segments"][0];
        assert_eq!(segment["id"], format!("{rid}_cloud_0"));
        assert_eq!(segment["startSample"], 16);
        assert_eq!(segment["endSample"], 320);
        assert_eq!(segment["machineRevision"], 2);
        assert_eq!(segment["final"], true);
        runtime
            .store
            .lock()
            .unwrap()
            .mutate(|state| {
                state.worker_epoch += 1;
                Ok(())
            })
            .unwrap();
        let stale = commit_cloud_update(
            &runtime,
            &sid,
            &rid,
            &CloudResume {
                cursor: 0,
                ordinal: 1,
            },
            Update {
                index: 0,
                start_sample: 0,
                end_sample: 320,
                text: "stale".into(),
                final_result: false,
            },
            epoch,
        )
        .unwrap_err();
        assert_eq!(stale, "STALE_WORKER_EPOCH");
    }

    #[test]
    fn cloud_sentence_final_uses_token_end_after_longer_provisional_progress() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "cloud sentence ranges");
        select_soniox(&runtime, true);
        let rid = runtime.add_run(&sid, "import").unwrap();
        runtime
            .update(&sid, |value| {
                value["runs"].as_array_mut().unwrap().last_mut().unwrap()["samples"] =
                    json!(20_000);
                Ok(())
            })
            .unwrap();
        let epoch = runtime.snapshot().unwrap().worker_epoch;
        let resume = CloudResume::default();
        commit_cloud_update(
            &runtime,
            &sid,
            &rid,
            &resume,
            Update {
                index: 0,
                start_sample: 0,
                end_sample: 20_000,
                text: "First sentence. Second".into(),
                final_result: false,
            },
            epoch,
        )
        .unwrap();
        commit_cloud_update(
            &runtime,
            &sid,
            &rid,
            &resume,
            Update {
                index: 0,
                start_sample: 0,
                end_sample: 8_000,
                text: "First sentence.".into(),
                final_result: true,
            },
            epoch,
        )
        .unwrap();
        commit_cloud_update(
            &runtime,
            &sid,
            &rid,
            &resume,
            Update {
                index: 1,
                start_sample: 9_000,
                end_sample: 15_000,
                text: "Second sentence.".into(),
                final_result: true,
            },
            epoch,
        )
        .unwrap();

        let session = runtime.session(&sid).unwrap();
        let segments = session["segments"].as_array().unwrap();
        assert_eq!(segments[0]["endSample"], 8_000);
        assert_eq!(segments[1]["startSample"], 9_000);
        assert!(segments[0]["endSample"].as_u64() <= segments[1]["startSample"].as_u64());
    }
    #[test]
    fn short_final_publishes_sample_boundary_before_worker_commit() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let state = runtime
            .dispatch(
                json!({"type":"createSession","commandId":uid(),"title":"boundary","mode":"live"}),
            )
            .unwrap();
        let sid = state.selected_session_id.unwrap();
        let rid = runtime.add_run(&sid, "microphone").unwrap();
        let mut archiver = Archiver::new(runtime.clone(), sid.clone(), rid.clone(), 16000).unwrap();
        archiver
            .frame(capture::Frame {
                start: 0,
                samples: vec![0.1; 3200],
            })
            .unwrap();
        archiver.finish().unwrap();
        let job = runtime.jobs().unwrap().pop().unwrap();
        assert_eq!(job.end, 3200);
        assert_eq!(runtime.session(&sid).unwrap()["runs"][0]["samples"], 3200);
        let epoch = runtime.snapshot().unwrap().worker_epoch;
        runtime.dispatch(json!({"type":"machine","commandId":uid(),"sessionId":sid,"segmentId":job.id,"runId":rid,"startSample":0,"endSample":3200,"text":"test","revision":1,"workerEpoch":epoch,"final":true})).unwrap();
    }
    #[test]
    fn a_microphone_delivering_only_zeros_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let state = runtime
            .dispatch(
                json!({"type":"createSession","commandId":uid(),"title":"muted","mode":"live"}),
            )
            .unwrap();
        let sid = state.selected_session_id.unwrap();
        let record = |samples: Vec<f32>| {
            let rid = runtime.add_run(&sid, "microphone").unwrap();
            let mut archiver = Archiver::new(runtime.clone(), sid.clone(), rid, 16000).unwrap();
            archiver.watch_for_muted_input();
            archiver
                .frame(capture::Frame { start: 0, samples })
                .unwrap();
            archiver.finish().unwrap();
            runtime.session(&sid).unwrap()["error"].clone()
        };
        // A real, quiet room still has a noise floor.
        assert!(record(
            (0..64_000)
                .map(|i| if i % 2 == 0 { 1e-4 } else { -1e-4 })
                .collect()
        )
        .is_null());
        assert_eq!(record(vec![0.0; 64_000]), MUTED_MICROPHONE);
    }
    #[test]
    fn corrupt_jobs_are_quarantined_and_valid_jobs_continue() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        fs::write(temp.path().join("jobs/bad.json"), b"{broken").unwrap();
        let job = Job {
            id: uid(),
            session: uid(),
            run: uid(),
            start: 0,
            end: 16000,
            final_result: true,
            attempts: 0,
            failed: false,
        };
        runtime.save_job(&job).unwrap();
        assert_eq!(runtime.jobs().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(temp.path().join("recovery/quarantined-jobs"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(runtime.jobs().unwrap().len(), 1);
    }
    #[test]
    fn recovery_clamps_ranges_published_ahead_of_durable_audio() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "power loss");
        let rid = runtime.add_run(&sid, "microphone").unwrap();
        let path = runtime.run_path(&sid, &rid).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![0u8; 32_000]).unwrap();
        let partial = |start, end, final_result| Job {
            id: format!("{rid}_{start}"),
            session: sid.clone(),
            run: rid.clone(),
            start,
            end,
            final_result,
            attempts: 0,
            failed: false,
        };
        runtime.enqueue(partial(0, 48_000, false)).unwrap();
        let epoch = runtime.snapshot().unwrap().worker_epoch;
        runtime.dispatch(json!({"type":"machine","commandId":uid(),"sessionId":sid,"segmentId":format!("{rid}_0"),"runId":rid,"startSample":0,"endSample":48_000,"text":"unsynced","revision":1,"workerEpoch":epoch,"final":false})).unwrap();
        runtime.dispatch(json!({"type":"addNote","commandId":uid(),"sessionId":sid,"segmentId":format!("{rid}_0"),"kind":"note","text":"n"})).unwrap();
        runtime
            .update(&sid, |s| {
                s["notes"][0]["sample"] = json!(40_000);
                Ok(())
            })
            .unwrap();
        runtime.save_job(&partial(8_000, 48_000, true)).unwrap();
        runtime.save_job(&partial(20_000, 48_000, true)).unwrap();

        runtime.recover().unwrap();
        let session = runtime.session(&sid).unwrap();
        assert_eq!(session["runs"][0]["state"], "interrupted");
        assert_eq!(session["runs"][0]["samples"], 16_000);
        assert_eq!(session["segments"][0]["endSample"], 16_000);
        assert_eq!(session["notes"][0]["sample"], 16_000);
        assert_eq!(session["recordingState"], "error");
        let jobs = runtime.jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].start, jobs[0].end), (8_000, 16_000));
    }
    #[test]
    fn pause_keeps_a_capture_failure_recorded_while_closing() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "device lost");
        let rid = runtime.add_run(&sid, "microphone").unwrap();
        runtime
            .update(&sid, |session| {
                session["recordingState"] = json!("recording");
                Ok(())
            })
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, closing, session_id) = (stop.clone(), runtime.clone(), sid.clone());
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            closing
                .close_run(&session_id, &rid, 0, "interrupted", Some("音频源已断开"))
                .unwrap();
        });
        *runtime.capture.lock().unwrap() = Some(Recording {
            session: sid.clone(),
            stop,
            thread,
        });
        runtime
            .dispatch(json!({"type":"pauseRecording","commandId":uid(),"sessionId":sid.clone()}))
            .unwrap();
        let session = runtime.session(&sid).unwrap();
        assert_eq!(session["recordingState"], "error");
        assert_eq!(session["error"], "音频源已断开");
    }
    #[test]
    fn switching_to_soniox_waits_for_pending_local_jobs() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "switch");
        let rid = runtime.add_run(&sid, "microphone").unwrap();
        let mut job = Job {
            id: format!("{rid}_0"),
            session: sid,
            run: rid,
            start: 0,
            end: 16_000,
            final_result: true,
            attempts: 0,
            failed: false,
        };
        runtime.save_job(&job).unwrap();
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.engine = "soniox".into();
        let switch = json!({"type":"settings","commandId":uid(),"settings":settings});
        assert!(runtime
            .dispatch(switch.clone())
            .unwrap_err()
            .contains("1 段"));
        assert_eq!(runtime.snapshot().unwrap().settings.engine, "qwen");
        job.failed = true;
        runtime.save_job(&job).unwrap();
        runtime.dispatch(switch).unwrap();
        assert_eq!(runtime.snapshot().unwrap().settings.engine, "soniox");
    }
    #[test]
    fn unprepared_model_cannot_open_capture_device() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let state = runtime
            .dispatch(
                json!({"type":"createSession","commandId":uid(),"title":"readiness","mode":"live"}),
            )
            .unwrap();
        let sid = state.selected_session_id.unwrap();
        assert!(runtime
            .start(&sid, "microphone")
            .unwrap_err()
            .contains("模型"));
        assert!(runtime.session(&sid).unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn whisper_is_preserved_but_cannot_report_ready_or_open_capture() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "whisper gate");
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.engine = "whisper".into();
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();

        let info = runtime.info();
        assert_eq!(info["modelReady"], false);
        assert_eq!(info["modelState"], "error");
        assert_eq!(info["modelError"], WHISPER_REALTIME_UNAVAILABLE);
        assert_eq!(runtime.snapshot().unwrap().settings.engine, "whisper");
        assert_eq!(
            runtime.start(&sid, "microphone").unwrap_err(),
            WHISPER_REALTIME_UNAVAILABLE
        );
        assert!(runtime.session(&sid).unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn exited_qwen_process_revokes_ready_and_allows_prepare_retry() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "qwen liveness");
        let settings = serde_json::to_value(runtime.snapshot().unwrap().settings).unwrap();
        let mut exited = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        exited.wait().unwrap();
        *runtime.model_child.lock().unwrap() = Some(exited);
        *runtime.model_status.lock().unwrap() = json!({"state":"ready","settings":settings});

        let info = runtime.info();
        assert_eq!(info["modelReady"], false);
        assert_eq!(info["modelState"], "error");
        assert_eq!(info["modelError"], QWEN_MODEL_EXITED);
        assert!(runtime.model_child.lock().unwrap().is_none());
        assert_eq!(
            runtime.start(&sid, "microphone").unwrap_err(),
            QWEN_MODEL_EXITED
        );
        assert!(runtime.session(&sid).unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty());

        runtime
            .dispatch(json!({"type":"prepareModel","commandId":uid()}))
            .unwrap();
        assert!(runtime.prepare_requested.load(Ordering::SeqCst));
    }
    #[test]
    fn qwen_tags() {
        assert_eq!(parse_qwen("language English<asr_text>Hello"), "Hello");
        assert_eq!(parse_qwen("<asr_text>你好"), "你好");
        assert_eq!(parse_qwen("Plain text"), "Plain text");
    }
    #[test]
    fn decoding_loops_collapse_but_real_repeats_stay() {
        let looped = format!("That your measure is accurate, right? As, {}", "as, ".repeat(300));
        assert_eq!(collapse_repetition(&looped), "That your measure is accurate, right? As");
        assert_eq!(collapse_repetition("It is very, very, very important."), "It is very, very, very important.");
        assert_eq!(collapse_repetition("thank you thank you thank you thank you thank you so much"), "thank you so much");
        assert_eq!(collapse_repetition("这个这个这个这个这个问题很重要"), "这个问题很重要");
        assert_eq!(collapse_repetition("我们来看第一个问题。"), "我们来看第一个问题。");
        assert_eq!(collapse_repetition("GDP grew 2 2 2 percent"), "GDP grew 2 2 2 percent");
        assert_eq!(collapse_repetition(""), "");
    }
    #[test]
    fn token_budget_follows_audio_length() {
        assert_eq!(qwen_token_budget(44), 48);
        assert_eq!(qwen_token_budget(44 + 8 * 32_000), 128);
        assert_eq!(qwen_token_budget(44 + 60 * 32_000), 256);
    }
    #[test]
    fn wav_header() {
        let data = wav_bytes(&[0, 0, 1, 0]);
        let reader = hound::WavReader::new(std::io::Cursor::new(data)).unwrap();
        assert_eq!(reader.duration(), 2);
        assert_eq!(reader.spec().sample_rate, 16000);
    }
    #[test]
    fn path_validation() {
        assert!(id("../../secret").is_err());
        assert!(id(&uid()).is_ok());
    }

    #[test]
    fn model_download_progress_is_exposed_and_never_persisted() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let initial = runtime.info();
        assert_eq!(initial["modelDownload"]["phase"], "idle");
        assert_eq!(
            initial["modelDownload"]["totalBytes"],
            crate::models::install_total_bytes()
        );

        *runtime.model_download.lock().unwrap() = json!({
            "phase":"downloading",
            "downloadedBytes":123,
            "totalBytes":456,
            "fileName":"model.bin",
            "error":Value::Null
        });
        let info = runtime.info();
        assert_eq!(info["modelDownload"]["downloadedBytes"], 123);
        assert_eq!(info["modelDownload"]["fileName"], "model.bin");
        let persisted_state = serde_json::to_string(&runtime.snapshot().unwrap()).unwrap();
        assert!(!persisted_state.contains("modelDownload"));
        assert!(!persisted_state.contains("model.bin"));
    }

    #[test]
    fn model_download_error_state_is_readable() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        *runtime.model_download.lock().unwrap() = json!({
            "phase":"error",
            "downloadedBytes":2048,
            "totalBytes":4096,
            "fileName":"model.bin",
            "error":"连接中断"
        });

        let info = runtime.info();
        assert_eq!(info["modelDownload"]["phase"], "error");
        assert_eq!(info["modelDownload"]["downloadedBytes"], 2048);
        assert_eq!(info["modelDownload"]["error"], "连接中断");
    }

    #[test]
    fn concurrent_model_install_is_rejected_before_network_access() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        runtime.model_installing.store(true, Ordering::SeqCst);

        let result = runtime.dispatch(json!({"type":"installModel","commandId":uid()}));

        assert!(result.unwrap_err().contains("正在下载或校验"));
        assert_eq!(runtime.info()["modelDownload"]["phase"], "idle");
        assert!(!temp.path().join("models").exists());
        runtime.model_installing.store(false, Ordering::SeqCst);
    }

    #[test]
    fn active_cloud_processing_rejects_model_install_before_network_access() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
        });
        *runtime.cloud.lock().unwrap() = Some(CloudTask {
            session: "cloud-session".into(),
            stop: stop.clone(),
            thread,
        });

        let result = runtime.dispatch(json!({"type":"installModel","commandId":uid()}));

        assert!(result.unwrap_err().contains("Soniox 转写正在进行"));
        assert!(!runtime.model_installing.load(Ordering::SeqCst));
        assert_eq!(runtime.info()["modelDownload"]["phase"], "idle");
        let task = runtime.cloud.lock().unwrap().take().unwrap();
        task.stop.store(true, Ordering::SeqCst);
        task.thread.join().unwrap();
    }

    #[test]
    fn local_info_rejects_truncated_standard_qwen_installation() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let executable = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        let executable = temp.path().join("resources/native/qwen").join(executable);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"test executable").unwrap();
        let models = temp.path().join("models/qwen3-asr-0.6b-q8");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join("Qwen3-ASR-0.6B-Q8_0.gguf"), b"model").unwrap();
        fs::write(models.join("mmproj-Qwen3-ASR-0.6B-Q8_0.gguf"), b"projector").unwrap();
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.engine = "qwen".into();
        settings.executable = executable.to_string_lossy().into_owned();
        settings.model_path = models
            .join("Qwen3-ASR-0.6B-Q8_0.gguf")
            .to_string_lossy()
            .into_owned();
        settings.mmproj_path = models
            .join("mmproj-Qwen3-ASR-0.6B-Q8_0.gguf")
            .to_string_lossy()
            .into_owned();
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();

        let info = runtime.info();
        assert_eq!(info["modelInstalled"], true);
        assert_eq!(info["localModelInstalled"], false);
    }

    #[test]
    fn soniox_info_accepts_existing_custom_qwen_files() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let executable = temp.path().join("custom/llama-server");
        let model = temp.path().join("custom/my-qwen.gguf");
        let mmproj = temp.path().join("custom/my-mmproj.gguf");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"test executable").unwrap();
        fs::write(&model, b"custom model").unwrap();
        fs::write(&mmproj, b"custom projector").unwrap();
        let mut settings = runtime.snapshot().unwrap().settings;
        settings.engine = "soniox".into();
        settings.executable = executable.to_string_lossy().into_owned();
        settings.model_path = model.to_string_lossy().into_owned();
        settings.mmproj_path = mmproj.to_string_lossy().into_owned();
        runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap();

        assert_eq!(runtime.info()["localModelInstalled"], true);
    }

    #[test]
    fn recording_and_settings_changes_are_blocked_during_model_install() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "install gate");
        runtime.model_installing.store(true, Ordering::SeqCst);
        let settings = runtime.snapshot().unwrap().settings;

        assert!(runtime
            .start(&sid, "microphone")
            .unwrap_err()
            .contains("正在安装"));
        assert!(runtime
            .dispatch(json!({"type":"settings","commandId":uid(),"settings":settings}))
            .unwrap_err()
            .contains("正在下载或校验"));
        let prepared = Arc::new(AtomicBool::new(false));
        let prepare_flag = prepared.clone();
        assert!(runtime
            .spawn_cloud_retry_with(sid, move || {
                prepare_flag.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .contains("正在安装"));
        assert!(!prepared.load(Ordering::SeqCst));
        runtime.model_installing.store(false, Ordering::SeqCst);
    }

    #[test]
    fn cloud_retry_rechecks_install_reservation_after_waiting_for_cloud_lock() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = isolated_runtime(temp.path());
        let sid = session(&runtime, "reservation race");
        let cloud_guard = runtime.cloud.lock().unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let thread_barrier = barrier.clone();
        let worker_runtime = runtime.clone();
        let prepared = Arc::new(AtomicBool::new(false));
        let prepare_flag = prepared.clone();
        let attempt = thread::spawn(move || {
            thread_barrier.wait();
            worker_runtime.spawn_cloud_retry_with(sid, move || {
                prepare_flag.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        barrier.wait();
        runtime.model_installing.store(true, Ordering::SeqCst);
        drop(cloud_guard);

        assert!(attempt.join().unwrap().unwrap_err().contains("正在安装"));
        assert!(!prepared.load(Ordering::SeqCst));
        assert!(runtime.cloud.lock().unwrap().is_none());
        runtime.model_installing.store(false, Ordering::SeqCst);
    }

    #[test]
    fn appearance_is_excluded_from_loaded_model_identity() {
        let light = json!({"engine":"qwen","executable":"server","modelPath":"model","mmprojPath":"projector","language":"en","theme":"light"});
        let dark = json!({"engine":"qwen","executable":"server","modelPath":"model","mmprojPath":"projector","language":"en","theme":"dark"});
        let other_language = json!({"engine":"qwen","executable":"server","modelPath":"model","mmprojPath":"projector","language":"zh","theme":"dark"});
        assert!(same_model_settings(&light, &dark));
        assert!(!same_model_settings(&light, &other_language));
    }

    #[test]
    fn vocabulary_builds_qwen_context_and_soniox_terms() {
        let settings =
            json!({"customVocabulary":["con yard → Cournot","Amartya Sen","amartya sen"]});
        assert_eq!(vocabulary_terms(&settings), vec!["Cournot", "Amartya Sen"]);
        let prompt = vocabulary_prompt(&settings);
        assert_eq!(prompt, "Vocabulary: Cournot, Amartya Sen.");
        assert!(!prompt.contains("exact spelling"));
        assert!(!prompt.contains("con yard"));
    }

    #[test]
    fn effective_vocabulary_prefers_lesson_then_course_then_global() {
        let state = json!({
            "settings":{"customVocabulary":["Global","Shared","same error → Global"]},
            "projects":[{"id":"p1","customVocabulary":["Course","shared","same error → Course"]}],
            "sessions":[{"id":"s1","projectId":"p1","customVocabulary":["Lesson","course","same error → Lesson"]}]
        });
        let settings = effective_session_settings(&state, "s1");
        assert_eq!(
            settings["customVocabulary"],
            json!([
                "Lesson",
                "course",
                "same error → Lesson",
                "shared",
                "Global"
            ])
        );
    }

    #[test]
    fn vocabulary_prompt_echo_is_removed_before_segmentation() {
        let settings = json!({"customVocabulary":["Tocqueville","con yard → Cournot"]});
        let legacy = "Equality produces two tendencies. Use this exact spelling when spoken: \"Tocqueville\".";
        assert_eq!(
            sanitize_vocabulary_prompt_leak(legacy, &settings),
            "Equality produces two tendencies."
        );
        let current = "Equality produces two tendencies. Vocabulary: Tocqueville, Cournot.";
        assert_eq!(
            sanitize_vocabulary_prompt_leak(current, &settings),
            "Equality produces two tendencies."
        );
        assert_eq!(
            sanitize_vocabulary_prompt_leak(
                "Tocqueville describes equality and freedom.",
                &settings
            ),
            "Tocqueville describes equality and freedom."
        );
    }

    #[test]
    fn qwen_final_text_splits_at_natural_boundaries_with_contiguous_ranges() {
        let job = Job {
            id: "run_0".into(),
            session: "session".into(),
            run: "run".into(),
            start: 1_000,
            end: 17_000,
            final_result: true,
            attempts: 0,
            failed: false,
        };
        let segments = split_final_text(&job, "First sentence. 第二句！ \"Third?\"");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].id, "run_0");
        assert_eq!(segments[1].id, "run_0_sentence_2");
        assert_eq!(segments[2].id, "run_0_sentence_3");
        assert_eq!(segments[0].start, job.start);
        assert_eq!(segments.last().unwrap().end, job.end);
        assert!(segments
            .windows(2)
            .all(|pair| pair[0].end == pair[1].start && pair[0].end >= pair[0].start));
        assert_eq!(segments[0].text, "First sentence.");
        assert_eq!(segments[1].text, "第二句！");
        assert_eq!(segments[2].text, "\"Third?\"");
    }

    #[test]
    fn qwen_weighted_ranges_cover_very_short_audio_monotonically() {
        let job = Job {
            id: "tiny".into(),
            session: "session".into(),
            run: "run".into(),
            start: 10,
            end: 12,
            final_result: true,
            attempts: 0,
            failed: false,
        };
        let segments = split_final_text(&job, "A. B. C.");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments.first().unwrap().start, 10);
        assert_eq!(segments.last().unwrap().end, 12);
        assert!(segments.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert!(segments.iter().all(|segment| segment.end >= segment.start));
    }

    #[test]
    fn periods_inside_numbers_and_abbreviations_do_not_split_sentences() {
        assert_eq!(
            natural_sentences("Pi is 3.14 roughly. Use e.g. a circle. Then stop."),
            ["Pi is 3.14 roughly.", "Use e.g. a circle.", "Then stop."]
        );
        assert_eq!(
            natural_sentences("Version 2. 5 more items."),
            ["Version 2. 5 more items."]
        );
        assert_eq!(
            natural_sentences("He said \"done.\" Next one"),
            ["He said \"done.\"", "Next one"]
        );
        assert_eq!(
            natural_sentences("第一句。第二句！第三句？end"),
            ["第一句。", "第二句！", "第三句？", "end"]
        );
    }

    #[test]
    fn edited_or_open_segments_are_finalised_without_splitting() {
        let mut state = State::default();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"Split","mode":"live"}),
        )
        .unwrap();
        let sid = state.selected_session_id.clone().unwrap();
        crate::domain::apply_command(&mut state, &json!({"type":"machine","sessionId":sid,"segmentId":"r_0","runId":"r","startSample":0,"endSample":32000,"text":"First part","revision":1,"workerEpoch":0,"final":false})).unwrap();
        assert!(!human_owned(&state.sessions[0], "r_0"));
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"beginEdit","sessionId":sid,"segmentId":"r_0"}),
        )
        .unwrap();
        assert!(human_owned(&state.sessions[0], "r_0"));
        let draft = state.sessions[0].drafts[0].id.clone();
        crate::domain::apply_command(&mut state, &json!({"type":"commitEdit","sessionId":sid,"draftId":draft,"text":"First part, edited","expectedUserSeq":0})).unwrap();
        assert!(human_owned(&state.sessions[0], "r_0"));
        assert!(!human_owned(&state.sessions[0], "other"));
    }

    #[test]
    fn silence_below_the_recorder_speech_threshold_skips_the_model() {
        let quiet: Vec<u8> = (0..16_000)
            .flat_map(|index| (if index % 2 == 0 { 200i16 } else { -200 }).to_le_bytes())
            .collect();
        assert!(!has_speech(&quiet));
        let mut click = quiet.clone();
        for sample in click[6400..7040].chunks_exact_mut(2) {
            sample.copy_from_slice(&2000i16.to_le_bytes());
        }
        assert!(!has_speech(&click));
        let mut voiced = quiet.clone();
        for sample in voiced[6400..12800].chunks_exact_mut(2) {
            sample.copy_from_slice(&2000i16.to_le_bytes());
        }
        assert!(has_speech(&voiced));
    }

    #[test]
    fn vocabulary_words_need_the_context_free_pass_to_agree() {
        let settings = json!({"customVocabulary":["Tocqueville","con yard → Cournot"]});
        assert!(mentions_vocabulary(
            "the Greeks idolized their Tocquevillians.",
            &settings
        ));
        assert!(!mentions_vocabulary(
            "the Greeks idolized their Olympians.",
            &settings
        ));
        // Swapped in for a different word: the plain pass wins.
        assert_eq!(
            verify_vocabulary(
                "the same way the Greeks idolized their Tocquevillians. Americans",
                "the same way the Greeks idolized their Olympians. Americans",
                &settings
            ),
            "the same way the Greeks idolized their Olympians. Americans"
        );
        // Emitted over trailing silence: dropped.
        assert_eq!(
            verify_vocabulary(
                "complete their K to twelve in a day. Tocqueville. Tocqueville. Tocqueville.",
                "complete their K to twelve in a day.",
                &settings
            ),
            "complete their K to twelve in a day."
        );
        assert_eq!(
            verify_vocabulary("Tocqueville. Tocqueville.", "", &settings),
            ""
        );
        // Genuinely spoken but spelled differently without context: the term is kept.
        assert_eq!(
            verify_vocabulary(
                "As Tocqueville wrote, equality spreads.",
                "As Tokeville wrote, equality spreads.",
                &settings
            ),
            "As Tocqueville wrote, equality spreads."
        );
        assert_eq!(
            verify_vocabulary("The Cournot model.", "The con yard model.", &settings),
            "The Cournot model."
        );
        // Words the context did not affect are left alone.
        assert_eq!(
            verify_vocabulary(
                "Tocqueville saw two tendencies.",
                "Tokeville saw too tendencies.",
                &settings
            ),
            "Tocqueville saw two tendencies."
        );
        let chinese = json!({"customVocabulary":["托克维尔"]});
        assert_eq!(verify_vocabulary("托克维尔。托克维尔。", "", &chinese), "");
        assert_eq!(
            verify_vocabulary("托克维尔认为平等。", "托克维尔认为平等。", &chinese),
            "托克维尔认为平等。"
        );
    }

    #[test]
    fn permanently_failed_local_jobs_stay_visible_after_later_success() {
        let mut session =
            json!({"recordingState":"stopped","gaps":[],"error":null,"inferenceState":"running"});
        settle_local_inference(&mut session, 2);
        assert_eq!(session["inferenceState"], "error");
        assert_eq!(session["error"], "有 2 段音频转写失败，可重试");
        settle_local_inference(&mut session, 0);
        assert_eq!(session["inferenceState"], "ready");
        assert!(session["error"].is_null());
    }

    #[test]
    fn auto_polish_schedule_lease_releases_keys_for_retry() {
        let scheduled = Arc::new(Mutex::new(HashSet::new()));
        {
            let _lease = AutoPolishLease::acquire(scheduled.clone(), "group".into()).unwrap();
            assert!(AutoPolishLease::acquire(scheduled.clone(), "group".into()).is_none());
            assert_eq!(scheduled.lock().unwrap().len(), 1);
        }
        assert!(scheduled.lock().unwrap().is_empty());
        assert!(AutoPolishLease::acquire(scheduled.clone(), "group".into()).is_some());
    }

    #[test]
    fn auto_polish_debounce_only_allows_the_latest_final_generation() {
        let generations = Arc::new(Mutex::new(HashMap::new()));
        let first = advance_auto_polish_generation(&generations, "session\0run").unwrap();
        let second = advance_auto_polish_generation(&generations, "session\0run").unwrap();
        assert!(!claim_auto_polish_generation(
            &generations,
            "session\0run",
            first
        ));
        assert!(claim_auto_polish_generation(
            &generations,
            "session\0run",
            second
        ));
        assert!(generations.lock().unwrap().is_empty());
    }

    #[test]
    fn auto_polish_backfill_uses_the_selected_courses_latest_final_segment() {
        let mut state = State::default();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"older","mode":"live"}),
        )
        .unwrap();
        let older = state.selected_session_id.clone().unwrap();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"machine","sessionId":older,"segmentId":"older-final","runId":"r1","startSample":0,"endSample":16_000,"text":"Older final.","revision":1,"workerEpoch":0,"final":true}),
        )
        .unwrap();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"selected","mode":"live"}),
        )
        .unwrap();
        let selected = state.selected_session_id.clone().unwrap();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"machine","sessionId":selected,"segmentId":"selected-final","runId":"r2","startSample":0,"endSample":16_000,"text":"Selected final.","revision":1,"workerEpoch":0,"final":true}),
        )
        .unwrap();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"machine","sessionId":selected,"segmentId":"selected-partial","runId":"r2","startSample":16_000,"endSample":24_000,"text":"Partial", "revision":1,"workerEpoch":0,"final":false}),
        )
        .unwrap();

        assert_eq!(
            auto_polish_backfill_trigger(&state),
            Some((selected.clone(), "selected-final".into()))
        );

        crate::domain::apply_command(
            &mut state,
            &json!({"type":"beginEdit","sessionId":selected.clone(),"segmentId":"selected-final"}),
        )
        .unwrap();
        assert_eq!(auto_polish_backfill_trigger(&state), None);

        {
            let selected_session = state
                .sessions
                .iter_mut()
                .find(|session| session.id == selected)
                .unwrap();
            selected_session.drafts.clear();
            let segment = selected_session
                .segments
                .iter_mut()
                .find(|segment| segment.id == "selected-final")
                .unwrap();
            segment.history.push(Some(crate::domain::Correction {
                base_machine_text: segment.machine_text.clone(),
                base_machine_revision: segment.machine_revision,
                text: segment.display_text.clone(),
                user_seq: 1,
                automatic: true,
            }));
            segment.history_index = 0;
            segment.user_seq = 1;
        }
        assert_eq!(auto_polish_backfill_trigger(&state), None);

        state
            .sessions
            .iter_mut()
            .find(|session| session.id == selected)
            .unwrap()
            .segments
            .iter_mut()
            .find(|segment| segment.id == "selected-final")
            .unwrap()
            .history[0]
            .as_mut()
            .unwrap()
            .automatic = false;
        assert_eq!(
            auto_polish_backfill_trigger(&state),
            Some((selected, "selected-final".into()))
        );
    }

    #[test]
    fn auto_polish_paragraph_selection_respects_gap_and_eight_segment_boundaries() {
        let mut state = State::default();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"paragraph","mode":"live"}),
        )
        .unwrap();
        let sid = state.selected_session_id.clone().unwrap();
        for index in 0..10u64 {
            let start = if index < 8 {
                index * 16_000
            } else {
                index * 16_000 + 90_000
            };
            crate::domain::apply_command(
                &mut state,
                &json!({"type":"machine","sessionId":sid,"segmentId":format!("s{index}"),"runId":"r1","startSample":start,"endSample":start + 16_000,"text":format!("fragment {index}"),"revision":1,"workerEpoch":0,"final":true}),
            )
            .unwrap();
        }
        let session = &state.sessions[0];
        assert_eq!(auto_polish_paragraph_bounds(session, 3), (0, 8));
        assert_eq!(auto_polish_paragraph_bounds(session, 8), (8, 10));
    }

    #[test]
    fn auto_polish_holds_the_newest_sentence_until_it_is_settled() {
        let mut state = State::default();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"tail","mode":"live"}),
        )
        .unwrap();
        let sid = state.selected_session_id.clone().unwrap();
        // A three-second pause inside one sentence still counts as adjacent.
        for (index, start) in [0u64, 64_000].into_iter().enumerate() {
            crate::domain::apply_command(
                &mut state,
                &json!({"type":"machine","sessionId":sid,"segmentId":format!("t{index}"),"runId":"r1","startSample":start,"endSample":start + 16_000,"text":format!("part {index}"),"revision":1,"workerEpoch":0,"final":true}),
            )
            .unwrap();
        }
        let session = &mut state.sessions[0];
        session.runs.push(crate::domain::Run {
            id: "r1".into(),
            source: "microphone".into(),
            engine: "qwen".into(),
            started_at: 0,
            ended_at: None,
            samples: 96_000,
            offset_ms: 0,
            state: "recording".into(),
        });
        assert_eq!(auto_polish_paragraph_bounds(session, 1), (0, 2));
        assert!(auto_polish_tail_open(session, 2));
        assert!(!auto_polish_tail_open(session, 1));

        session.runs[0].state = "closed".into();
        assert!(!auto_polish_tail_open(session, 2));
        let snapshot = state.clone();
        assert_eq!(
            auto_polish_run_tail(&snapshot, &sid, "r1"),
            Some("t1".into())
        );

        // A live hypothesis after the tail keeps it open even after the run closes.
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"machine","sessionId":sid,"segmentId":"live","runId":"r1","startSample":80_000,"endSample":90_000,"text":"still","revision":1,"workerEpoch":0,"final":false}),
        )
        .unwrap();
        assert!(auto_polish_tail_open(&state.sessions[0], 2));
    }

    #[test]
    fn auto_polish_paragraph_selection_respects_character_boundary() {
        let mut state = State::default();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"paragraph size","mode":"live"}),
        )
        .unwrap();
        let sid = state.selected_session_id.clone().unwrap();
        for (index, text) in ["a".repeat(7_000), "b".repeat(6_000)]
            .into_iter()
            .enumerate()
        {
            let start = index as u64 * 16_000;
            crate::domain::apply_command(
                &mut state,
                &json!({"type":"machine","sessionId":sid,"segmentId":format!("large{index}"),"runId":"r1","startSample":start,"endSample":start + 16_000,"text":text,"revision":1,"workerEpoch":0,"final":true}),
            )
            .unwrap();
        }
        let session = &state.sessions[0];
        assert_eq!(auto_polish_paragraph_bounds(session, 0), (0, 1));
        assert_eq!(auto_polish_paragraph_bounds(session, 1), (1, 2));
    }

    #[test]
    fn auto_polish_rolling_window_rechecks_automatic_tail_and_stops_after_undo() {
        let mut state = State::default();
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"createSession","title":"rolling paragraph","mode":"live"}),
        )
        .unwrap();
        let sid = state.selected_session_id.clone().unwrap();
        for index in 0..9u64 {
            let start = index * 16_000;
            crate::domain::apply_command(
                &mut state,
                &json!({"type":"machine","sessionId":sid,"segmentId":format!("r{index}"),"runId":"r1","startSample":start,"endSample":start + 16_000,"text":format!("fragment {index}"),"revision":1,"workerEpoch":0,"final":true}),
            )
            .unwrap();
        }
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"polishSegments","sessionId":sid,"sources":[
                {"id":"r7","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"fragment 7"}
            ],"segments":[{"sourceStart":0,"sourceEnd":1,"text":"Fragment 7."}]}),
        )
        .unwrap();
        assert_eq!(auto_polish_paragraph_bounds(&state.sessions[0], 8), (6, 9));

        let user_seq = state.sessions[0].segments[7].user_seq;
        crate::domain::apply_command(
            &mut state,
            &json!({"type":"undo","sessionId":sid,"segmentId":"r7","expectedUserSeq":user_seq}),
        )
        .unwrap();
        assert_eq!(auto_polish_paragraph_bounds(&state.sessions[0], 8), (8, 9));
    }
}
