use base64::{engine::general_purpose::STANDARD, Engine};
use lectureedit_merge::merge3;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MAX_TITLE_BYTES: usize = 4 * 1024;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_ITEMS: usize = 100_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub engine: String,
    pub executable: String,
    pub model_path: String,
    pub mmproj_path: String,
    pub language: String,
    #[serde(default)]
    pub custom_vocabulary: Vec<String>,
    #[serde(default)]
    pub cloud_consent: bool,
    #[serde(default)]
    pub auto_polish: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "system".into()
}

fn vocabulary_parts(entry: &str) -> (Option<&str>, &str) {
    let trimmed = entry.trim();
    let split = trimmed.split_once('→').or_else(|| trimmed.split_once("=>"));
    match split {
        Some((heard, preferred)) if !heard.trim().is_empty() && !preferred.trim().is_empty() => {
            (Some(heard.trim()), preferred.trim())
        }
        _ => (None, trimmed),
    }
}

fn validate_vocabulary(entries: &[String]) -> Result<(), String> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        count += 1;
        bytes = bytes.saturating_add(trimmed.len());
        let (heard, preferred) = vocabulary_parts(trimmed);
        if trimmed.len() > 200
            || trimmed.chars().any(char::is_control)
            || preferred.is_empty()
            || heard.is_some_and(str::is_empty)
        {
            return Err("INVALID_VOCABULARY".into());
        }
    }
    if count > 200 || bytes > 10_000 {
        return Err("INVALID_VOCABULARY".into());
    }
    Ok(())
}

fn replace_spelling(text: &str, heard: &str, preferred: &str) -> String {
    if heard.is_empty() || heard == preferred {
        return text.to_owned();
    }
    let ascii_word = heard.is_ascii()
        && heard
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && heard
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while cursor < text.len() {
        let found = text[cursor..].char_indices().find_map(|(offset, _)| {
            let start = cursor + offset;
            let end = start.checked_add(heard.len())?;
            if end > text.len() || !text.is_char_boundary(end) {
                return None;
            }
            let candidate = &text[start..end];
            let matches = if heard.is_ascii() {
                candidate.eq_ignore_ascii_case(heard)
            } else {
                candidate == heard
            };
            if !matches {
                return None;
            }
            if ascii_word {
                let joined_left = text[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
                let joined_right = text[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
                if joined_left || joined_right {
                    return None;
                }
            }
            Some((start, end))
        });
        let Some((start, end)) = found else {
            output.push_str(&text[cursor..]);
            break;
        };
        output.push_str(&text[cursor..start]);
        output.push_str(preferred);
        cursor = end;
    }
    output
}

fn apply_vocabulary(text: &str, entries: &[String]) -> String {
    entries.iter().fold(text.to_owned(), |current, entry| {
        let (heard, preferred) = vocabulary_parts(entry);
        match heard {
            Some(heard) => replace_spelling(&current, heard, preferred),
            None => current,
        }
    })
}

fn vocabulary_identity(entry: &str) -> String {
    let (heard, preferred) = vocabulary_parts(entry);
    match heard {
        Some(heard) => format!("mapping:{}", heard.to_lowercase()),
        None => format!("term:{}", preferred.to_lowercase()),
    }
}

fn effective_vocabulary(state: &State, session_id: &str) -> Result<Vec<String>, String> {
    let session = session(state, session_id)?;
    let project_entries = session
        .project_id
        .as_deref()
        .and_then(|id| state.projects.iter().find(|project| project.id == id))
        .map(|project| project.custom_vocabulary.as_slice())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let mut bytes = 0usize;
    for entry in session
        .custom_vocabulary
        .iter()
        .chain(project_entries)
        .chain(&state.settings.custom_vocabulary)
    {
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
    Ok(entries)
}

impl Settings {
    fn transcription_eq(&self, other: &Self) -> bool {
        self.engine == other.engine
            && self.executable == other.executable
            && self.model_path == other.model_path
            && self.mmproj_path == other.mmproj_path
            && self.language == other.language
            && self.custom_vocabulary == other.custom_vocabulary
            && self.cloud_consent == other.cloud_consent
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: "qwen".into(),
            executable: String::new(),
            model_path: String::new(),
            mmproj_path: String::new(),
            language: "auto".into(),
            custom_vocabulary: Vec::new(),
            cloud_consent: false,
            auto_polish: false,
            theme: default_theme(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub schema_version: u32,
    #[serde(default)]
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    pub selected_session_id: Option<String>,
    pub settings: Settings,
    pub worker_epoch: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema_version: 1,
            projects: Vec::new(),
            sessions: Vec::new(),
            selected_session_id: None,
            settings: Settings::default(),
            worker_epoch: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    #[serde(default)]
    pub custom_vocabulary: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub title: String,
    pub created_at: i64,
    pub mode: String,
    pub recording_state: String,
    pub inference_state: String,
    pub segments: Vec<Segment>,
    pub drafts: Vec<Draft>,
    pub notes: Vec<Note>,
    pub runs: Vec<Run>,
    pub gaps: Vec<Gap>,
    pub operation_seq: u64,
    pub error: Option<String>,
    #[serde(default)]
    pub custom_vocabulary: Vec<String>,
}

impl Session {
    pub fn new(title: String, mode: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            project_id: None,
            title,
            created_at: now_ms(),
            mode,
            recording_state: "stopped".into(),
            inference_state: "idle".into(),
            segments: Vec::new(),
            drafts: Vec::new(),
            notes: Vec::new(),
            runs: Vec::new(),
            gaps: Vec::new(),
            operation_seq: 0,
            error: None,
            custom_vocabulary: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub base_machine_text: String,
    pub base_machine_revision: u64,
    pub text: String,
    pub user_seq: u64,
    #[serde(default)]
    pub automatic: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MachineHypothesis {
    pub revision: u64,
    pub worker_epoch: u64,
    pub text: String,
    #[serde(rename = "final")]
    pub final_: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: String,
    pub run_id: String,
    pub start_sample: u64,
    pub end_sample: u64,
    pub machine_text: String,
    pub machine_revision: u64,
    #[serde(rename = "final")]
    pub final_: bool,
    pub worker_epoch: u64,
    pub user_seq: u64,
    pub display_text: String,
    pub pending_machine: Option<String>,
    pub corrected: bool,
    pub history: Vec<Option<Correction>>,
    pub history_index: i64,
    #[serde(default)]
    pub machine_history: Vec<MachineHypothesis>,
}

impl Segment {
    pub fn has_human_history(&self) -> bool {
        (!self.history.is_empty() && self.history_index != self.history.len() as i64 - 1)
            || self.history.iter().any(|entry| {
                entry
                    .as_ref()
                    .is_none_or(|correction| !correction.automatic)
            })
    }

    pub fn correction(&self) -> Option<&Correction> {
        usize::try_from(self.history_index)
            .ok()
            .and_then(|index| self.history.get(index))
            .and_then(Option::as_ref)
    }

    pub fn projection(&self) -> Projection {
        let Some(correction) = self.correction() else {
            return Projection {
                text: self.machine_text.clone(),
                pending_machine: None,
            };
        };
        let mut visible = correction.text.clone();
        let mut pending = None;
        for hypothesis in self
            .machine_history
            .iter()
            .filter(|item| item.revision > correction.base_machine_revision)
        {
            let candidate = merge3(
                &correction.base_machine_text,
                &correction.text,
                &hypothesis.text,
            );
            if candidate.conflict {
                pending = Some(hypothesis.text.clone());
            } else {
                visible = candidate.text;
                pending = None;
            }
        }
        Projection {
            text: visible,
            pending_machine: pending,
        }
    }

    pub fn refresh_projection(&mut self) {
        let projection = self.projection();
        self.display_text = projection.text;
        self.pending_machine = projection.pending_machine;
        self.corrected = self.correction().is_some();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub text: String,
    pub pending_machine: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    pub segment_id: String,
    pub base_machine_text: String,
    pub base_machine_revision: u64,
    pub snapshot_text: String,
    pub expected_user_seq: u64,
    pub text: String,
    pub revision: u64,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub segment_id: String,
    pub run_id: String,
    pub sample: u64,
    pub kind: String,
    pub text: String,
    pub source_label: Option<String>,
    pub image_data: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub source: String,
    #[serde(default = "default_run_engine")]
    pub engine: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub samples: u64,
    pub offset_ms: i64,
    pub state: String,
}

fn default_run_engine() -> String {
    "qwen".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub run_id: String,
    pub start_sample: u64,
    pub end_sample: u64,
    pub reason: String,
}

pub fn apply_command(state: &mut State, command: &Value) -> Result<(), String> {
    let kind = string(command, "type")?;
    match kind {
        "snapshot" => Ok(()),
        "createProject" => {
            let title = valid_title(command)?;
            let id = loop {
                let candidate = Uuid::new_v4().to_string();
                if state.projects.iter().all(|project| project.id != candidate) {
                    break candidate;
                }
            };
            state.projects.push(Project {
                id,
                title: title.to_string(),
                created_at: now_ms(),
                custom_vocabulary: Vec::new(),
            });
            Ok(())
        }
        "renameProject" => {
            let title = valid_title(command)?;
            let project_id = string(command, "projectId")?;
            let project = state
                .projects
                .iter_mut()
                .find(|project| project.id == project_id)
                .ok_or("PROJECT_NOT_FOUND")?;
            project.title = title.to_string();
            Ok(())
        }
        "moveSession" => {
            let project_id = nullable_string(command, "projectId")?.map(str::to_string);
            if let Some(project_id) = project_id.as_deref() {
                project(state, project_id)?;
            }
            let item = session_mut(state, string(command, "sessionId")?)?;
            item.project_id = project_id;
            bump(item);
            Ok(())
        }
        "createSession" => {
            let mode = string(command, "mode")?;
            if !matches!(mode, "live" | "demo") {
                return Err("INVALID_SESSION_MODE".into());
            }
            let title = valid_title(command)?;
            let project_id = optional_nullable_string(command, "projectId")?.map(str::to_string);
            if let Some(project_id) = project_id.as_deref() {
                project(state, project_id)?;
            }
            let mut session = Session::new(title.to_string(), mode.to_string());
            while state.sessions.iter().any(|item| item.id == session.id) {
                session.id = Uuid::new_v4().to_string();
            }
            session.project_id = project_id;
            state.selected_session_id = Some(session.id.clone());
            state.sessions.push(session);
            Ok(())
        }
        "selectSession" => {
            let id = string(command, "sessionId")?;
            session(state, id)?;
            state.selected_session_id = Some(id.to_string());
            Ok(())
        }
        "renameSession" => {
            let title = valid_title(command)?;
            let item = session_mut(state, string(command, "sessionId")?)?;
            item.title = title.to_string();
            bump(item);
            Ok(())
        }
        "beginEdit" => begin_edit(state, command),
        "saveDraft" => save_draft(state, command),
        "commitEdit" => commit_edit(state, command),
        "closeDraft" => set_draft_state(state, command, "closed"),
        "discardDraft" => set_draft_state(state, command, "discarded"),
        "resolve" => resolve(state, command),
        "undo" => move_history(state, command, -1),
        "redo" => move_history(state, command, 1),
        "addNote" => add_note(state, command),
        "removeNote" => remove_note(state, command),
        "settings" => {
            let settings: Settings =
                serde_json::from_value(command.get("settings").cloned().ok_or("MISSING_SETTINGS")?)
                    .map_err(|_| "INVALID_SETTINGS")?;
            if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
                return Err("INVALID_THEME".into());
            }
            validate_vocabulary(&settings.custom_vocabulary)?;
            let project_update = match (command.get("projectId"), command.get("projectVocabulary"))
            {
                (Some(Value::String(id)), Some(value)) => {
                    project(state, id)?;
                    let entries: Vec<String> =
                        serde_json::from_value(value.clone()).map_err(|_| "INVALID_VOCABULARY")?;
                    validate_vocabulary(&entries)?;
                    Some((id.clone(), entries))
                }
                (None, None) => None,
                _ => return Err("INVALID_VOCABULARY_SCOPE".into()),
            };
            let session_update = match (command.get("sessionId"), command.get("sessionVocabulary"))
            {
                (Some(Value::String(id)), Some(value)) => {
                    session(state, id)?;
                    let entries: Vec<String> =
                        serde_json::from_value(value.clone()).map_err(|_| "INVALID_VOCABULARY")?;
                    validate_vocabulary(&entries)?;
                    Some((id.clone(), entries))
                }
                (None, None) => None,
                _ => return Err("INVALID_VOCABULARY_SCOPE".into()),
            };
            let scoped_changed = project_update.as_ref().is_some_and(|(id, entries)| {
                state
                    .projects
                    .iter()
                    .find(|project| &project.id == id)
                    .is_none_or(|project| &project.custom_vocabulary != entries)
            }) || session_update.as_ref().is_some_and(|(id, entries)| {
                state
                    .sessions
                    .iter()
                    .find(|session| &session.id == id)
                    .is_none_or(|session| &session.custom_vocabulary != entries)
            });
            if state.settings != settings || scoped_changed {
                if !state.settings.transcription_eq(&settings) || scoped_changed {
                    state.worker_epoch =
                        state.worker_epoch.checked_add(1).ok_or("EPOCH_OVERFLOW")?;
                }
                state.settings = settings;
                if let Some((id, entries)) = project_update {
                    state
                        .projects
                        .iter_mut()
                        .find(|project| project.id == id)
                        .ok_or("PROJECT_NOT_FOUND")?
                        .custom_vocabulary = entries;
                }
                if let Some((id, entries)) = session_update {
                    let session = state
                        .sessions
                        .iter_mut()
                        .find(|session| session.id == id)
                        .ok_or("SESSION_NOT_FOUND")?;
                    session.custom_vocabulary = entries;
                    bump(session);
                }
            }
            Ok(())
        }
        "machine" => machine(state, command),
        "machineSegments" => machine_segments(state, command),
        "polishSegment" => polish_segment(state, command),
        "polishSegments" => polish_segments(state, command),
        "demoTick" => demo_tick(state, command),
        "importSession" => {
            let mut imported: Session =
                serde_json::from_value(command.get("session").cloned().ok_or("MISSING_SESSION")?)
                    .map_err(|_| "INVALID_SESSION")?;
            if state.sessions.iter().any(|item| item.id == imported.id) {
                return Err("SESSION_EXISTS".into());
            }
            imported.project_id = None;
            validate_session(&imported)?;
            state.selected_session_id = Some(imported.id.clone());
            state.sessions.push(imported);
            Ok(())
        }
        _ => Err("UNKNOWN_COMMAND".into()),
    }
}

fn begin_edit(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let segment = item
        .segments
        .iter()
        .find(|segment| segment.id == string(command, "segmentId").unwrap_or_default())
        .ok_or("SEGMENT_NOT_FOUND")?;
    let (base_machine_text, base_machine_revision) = segment
        .correction()
        .map(|correction| {
            (
                correction.base_machine_text.clone(),
                correction.base_machine_revision,
            )
        })
        .unwrap_or_else(|| (segment.machine_text.clone(), segment.machine_revision));
    item.drafts.push(Draft {
        id: Uuid::new_v4().to_string(),
        segment_id: segment.id.clone(),
        base_machine_text,
        base_machine_revision,
        snapshot_text: segment.display_text.clone(),
        expected_user_seq: segment.user_seq,
        text: segment.display_text.clone(),
        revision: 0,
        state: "open".into(),
    });
    bump(item);
    Ok(())
}

fn save_draft(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let id = string(command, "draftId")?;
    let revision = uint(command, "revision")?;
    let text = string(command, "text")?;
    if text.len() > MAX_TEXT_BYTES * 2 {
        return Err("DRAFT_TOO_LARGE".into());
    }
    let draft = item
        .drafts
        .iter_mut()
        .find(|draft| draft.id == id)
        .ok_or("DRAFT_NOT_FOUND")?;
    if draft.state != "open" {
        return Err("DRAFT_CLOSED".into());
    }
    if revision < draft.revision {
        return Ok(());
    }
    if revision == draft.revision {
        if text != draft.text {
            return Err("DRAFT_REVISION_REUSED".into());
        }
        return Ok(());
    }
    draft.text = text.to_string();
    draft.revision = revision;
    bump(item);
    Ok(())
}

fn commit_edit(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let draft_id = string(command, "draftId")?;
    let text = string(command, "text")?.to_string();
    if text.len() > MAX_TEXT_BYTES * 2 {
        return Err("DRAFT_TOO_LARGE".into());
    }
    let expected = uint(command, "expectedUserSeq")?;
    let draft_index = item
        .drafts
        .iter()
        .position(|draft| draft.id == draft_id)
        .ok_or("DRAFT_NOT_FOUND")?;
    if item.drafts[draft_index].state != "open" {
        return Err("DRAFT_CLOSED".into());
    }
    let segment_id = item.drafts[draft_index].segment_id.clone();
    let segment_index = item
        .segments
        .iter()
        .position(|segment| segment.id == segment_id)
        .ok_or("SEGMENT_NOT_FOUND")?;
    if expected != item.drafts[draft_index].expected_user_seq
        || expected != item.segments[segment_index].user_seq
    {
        item.drafts[draft_index].text = text;
        return Err("HUMAN_VERSION_CONFLICT".into());
    }
    let draft = item.drafts[draft_index].clone();
    let segment = &mut item.segments[segment_index];
    if text != draft.snapshot_text {
        let keep = usize::try_from(segment.history_index + 1).unwrap_or(0);
        segment.history.truncate(keep);
        segment.user_seq += 1;
        segment.history.push(Some(Correction {
            base_machine_text: draft.base_machine_text,
            base_machine_revision: draft.base_machine_revision,
            text: text.clone(),
            user_seq: segment.user_seq,
            automatic: false,
        }));
        segment.history_index = segment.history.len() as i64 - 1;
        segment.refresh_projection();
    }
    item.drafts[draft_index].text = text;
    item.drafts[draft_index].state = "committed".into();
    bump(item);
    Ok(())
}

fn set_draft_state(state: &mut State, command: &Value, target: &str) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let id = string(command, "draftId")?;
    let draft = item
        .drafts
        .iter_mut()
        .find(|draft| draft.id == id)
        .ok_or("DRAFT_NOT_FOUND")?;
    if target == "discarded" && draft.state == "committed" {
        return Err("ALREADY_COMMITTED".into());
    }
    if target == "discarded" {
        draft.text.clear();
    }
    if draft.state == "open" || target == "discarded" {
        draft.state = target.into();
        bump(item);
    }
    Ok(())
}

fn resolve(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let id = string(command, "segmentId")?;
    let expected_user = uint(command, "expectedUserSeq")?;
    let expected_machine = uint(command, "expectedMachineRevision")?;
    let action = string(command, "action")?;
    let segment = item
        .segments
        .iter_mut()
        .find(|segment| segment.id == id)
        .ok_or("SEGMENT_NOT_FOUND")?;
    if segment.user_seq != expected_user || segment.machine_revision != expected_machine {
        return Err("RESOLUTION_VERSION_CONFLICT".into());
    }
    if !matches!(action, "keep" | "machine") {
        return Err("INVALID_RESOLUTION".into());
    }
    let visible = command
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or(&segment.display_text)
        .to_string();
    if visible.len() > MAX_TEXT_BYTES * 2 {
        return Err("RESOLUTION_TOO_LARGE".into());
    }
    let keep = usize::try_from(segment.history_index + 1).unwrap_or(0);
    segment.history.truncate(keep);
    segment.user_seq += 1;
    if action == "keep" {
        segment.history.push(Some(Correction {
            base_machine_text: segment.machine_text.clone(),
            base_machine_revision: segment.machine_revision,
            text: visible,
            user_seq: segment.user_seq,
            automatic: false,
        }));
    } else {
        segment.history.push(None);
    }
    segment.history_index = segment.history.len() as i64 - 1;
    segment.refresh_projection();
    bump(item);
    Ok(())
}

fn move_history(state: &mut State, command: &Value, direction: i64) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let id = string(command, "segmentId")?;
    let expected = uint(command, "expectedUserSeq")?;
    let segment = item
        .segments
        .iter_mut()
        .find(|segment| segment.id == id)
        .ok_or("SEGMENT_NOT_FOUND")?;
    if expected != segment.user_seq {
        return Err("HUMAN_VERSION_CONFLICT".into());
    }
    let next = segment.history_index + direction;
    if next < -1 || next >= segment.history.len() as i64 {
        return Err(if direction < 0 {
            "NOTHING_TO_UNDO"
        } else {
            "NOTHING_TO_REDO"
        }
        .into());
    }
    segment.history_index = next;
    segment.user_seq += 1;
    segment.refresh_projection();
    bump(item);
    Ok(())
}

fn add_note(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let segment_id = string(command, "segmentId")?;
    let kind = string(command, "kind")?;
    if !matches!(kind, "note" | "example" | "formula" | "image") {
        return Err("INVALID_NOTE".into());
    }
    let segment = item
        .segments
        .iter()
        .find(|segment| segment.id == segment_id)
        .ok_or("SEGMENT_NOT_FOUND")?;
    let image_data = command
        .get("imageData")
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = string(command, "text")?;
    let source_label = command
        .get("sourceLabel")
        .and_then(Value::as_str)
        .map(str::to_string);
    if text.len() > MAX_TEXT_BYTES
        || source_label
            .as_ref()
            .is_some_and(|label| label.len() > MAX_TITLE_BYTES)
        || (kind == "image"
            && image_data
                .as_deref()
                .is_none_or(|data| validate_image_data(data).is_err()))
        || (kind != "image" && image_data.is_some())
    {
        return Err("INVALID_NOTE".into());
    }
    item.notes.push(Note {
        id: Uuid::new_v4().to_string(),
        segment_id: segment.id.clone(),
        run_id: segment.run_id.clone(),
        sample: segment.start_sample,
        kind: kind.into(),
        text: text.to_string(),
        source_label,
        image_data,
    });
    bump(item);
    Ok(())
}

fn remove_note(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let id = string(command, "noteId")?;
    let before = item.notes.len();
    item.notes.retain(|note| note.id != id);
    if before == item.notes.len() {
        return Err("NOTE_NOT_FOUND".into());
    }
    bump(item);
    Ok(())
}

fn machine(state: &mut State, command: &Value) -> Result<(), String> {
    let epoch = uint(command, "workerEpoch")?;
    if epoch != state.worker_epoch {
        return Ok(());
    }
    let session_id = string(command, "sessionId")?;
    let vocabulary = effective_vocabulary(state, session_id)?;
    let text = apply_vocabulary(string(command, "text")?, &vocabulary);
    let item = session_mut(state, session_id)?;
    let id = string(command, "segmentId")?;
    let run_id = string(command, "runId")?;
    let start = uint(command, "startSample")?;
    let end = uint(command, "endSample")?;
    let revision = uint(command, "revision")?;
    let final_ = boolean(command, "final")?;
    let allow_final_range_shrink = final_
        && command
            .get("allowFinalRangeShrink")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if end < start || revision == 0 || text.len() > MAX_TEXT_BYTES {
        return Err("INVALID_HYPOTHESIS".into());
    }
    if let Some(segment) = item.segments.iter_mut().find(|segment| segment.id == id) {
        if segment.run_id != run_id || segment.start_sample != start {
            return Err("SEGMENT_ID_REUSED".into());
        }
        if end < segment.end_sample && !allow_final_range_shrink {
            return Err("AUDIO_RANGE_SHRANK".into());
        }
        if segment.final_ || revision <= segment.machine_revision {
            return Ok(());
        }
        segment.end_sample = end;
        segment.machine_text = text.clone();
        segment.machine_revision = revision;
        segment.final_ = final_;
        segment.worker_epoch = epoch;
        segment.machine_history.push(MachineHypothesis {
            revision,
            worker_epoch: epoch,
            text,
            final_,
        });
        segment.refresh_projection();
    } else {
        item.segments.push(Segment {
            id: id.into(),
            run_id: run_id.into(),
            start_sample: start,
            end_sample: end,
            machine_text: text.clone(),
            machine_revision: revision,
            final_,
            worker_epoch: epoch,
            user_seq: 0,
            display_text: text.clone(),
            pending_machine: None,
            corrected: false,
            history: Vec::new(),
            history_index: -1,
            machine_history: vec![MachineHypothesis {
                revision,
                worker_epoch: epoch,
                text,
                final_,
            }],
        });
    }
    bump(item);
    Ok(())
}

struct FinalMachineSegment {
    id: String,
    start: u64,
    end: u64,
    text: String,
    revision: u64,
}

fn machine_segments(state: &mut State, command: &Value) -> Result<(), String> {
    let epoch = uint(command, "workerEpoch")?;
    if epoch != state.worker_epoch {
        return Ok(());
    }
    let run_id = string(command, "runId")?.to_owned();
    let session_id = string(command, "sessionId")?;
    let vocabulary = effective_vocabulary(state, session_id)?;
    let values = command
        .get("segments")
        .and_then(Value::as_array)
        .ok_or("INVALID_HYPOTHESIS")?;
    if values.is_empty() || values.len() > MAX_ITEMS {
        return Err("INVALID_HYPOTHESIS".into());
    }
    let mut segments = Vec::with_capacity(values.len());
    let mut ids = HashSet::new();
    for value in values {
        let id = string(value, "id")?.to_owned();
        let start = uint(value, "startSample")?;
        let end = uint(value, "endSample")?;
        let revision = uint(value, "revision")?;
        let text = apply_vocabulary(string(value, "text")?, &vocabulary);
        if !safe_id(&id)
            || !ids.insert(id.clone())
            || end < start
            || revision == 0
            || text.len() > MAX_TEXT_BYTES
            || segments
                .last()
                .is_some_and(|previous: &FinalMachineSegment| previous.end != start)
        {
            return Err("INVALID_HYPOTHESIS".into());
        }
        segments.push(FinalMachineSegment {
            id,
            start,
            end,
            text,
            revision,
        });
    }

    let item = session_mut(state, string(command, "sessionId")?)?;
    let first_exists = item
        .segments
        .iter()
        .any(|segment| segment.id == segments[0].id);
    let additional = segments.len().saturating_sub(usize::from(first_exists));
    if item.segments.len().saturating_add(additional) > MAX_ITEMS {
        return Err("INVALID_HYPOTHESIS".into());
    }
    let first = &segments[0];
    if let Some(existing) = item.segments.iter().find(|segment| segment.id == first.id) {
        if existing.final_ {
            let already_applied = segments.iter().all(|expected| {
                item.segments.iter().any(|actual| {
                    actual.id == expected.id
                        && actual.run_id == run_id
                        && actual.start_sample == expected.start
                        && actual.end_sample == expected.end
                        && actual.machine_text == expected.text
                        && actual.final_
                })
            });
            return if already_applied {
                Ok(())
            } else {
                Err("SEGMENT_ID_REUSED".into())
            };
        }
        if existing.run_id != run_id
            || existing.start_sample != first.start
            || first.revision <= existing.machine_revision
        {
            return Err("SEGMENT_ID_REUSED".into());
        }
    }
    if segments
        .iter()
        .skip(1)
        .any(|expected| item.segments.iter().any(|actual| actual.id == expected.id))
    {
        return Err("SEGMENT_ID_REUSED".into());
    }

    if let Some(existing) = item
        .segments
        .iter_mut()
        .find(|segment| segment.id == first.id)
    {
        existing.end_sample = first.end;
        existing.machine_text = first.text.clone();
        existing.machine_revision = first.revision;
        existing.final_ = true;
        existing.worker_epoch = epoch;
        existing.machine_history.push(MachineHypothesis {
            revision: first.revision,
            worker_epoch: epoch,
            text: first.text.clone(),
            final_: true,
        });
        existing.refresh_projection();
    } else {
        item.segments.push(new_final_segment(first, &run_id, epoch));
    }
    for segment in &segments[1..] {
        item.segments
            .push(new_final_segment(segment, &run_id, epoch));
    }
    bump(item);
    Ok(())
}

fn new_final_segment(segment: &FinalMachineSegment, run_id: &str, epoch: u64) -> Segment {
    Segment {
        id: segment.id.clone(),
        run_id: run_id.to_owned(),
        start_sample: segment.start,
        end_sample: segment.end,
        machine_text: segment.text.clone(),
        machine_revision: segment.revision,
        final_: true,
        worker_epoch: epoch,
        user_seq: 0,
        display_text: segment.text.clone(),
        pending_machine: None,
        corrected: false,
        history: Vec::new(),
        history_index: -1,
        machine_history: vec![MachineHypothesis {
            revision: segment.revision,
            worker_epoch: epoch,
            text: segment.text.clone(),
            final_: true,
        }],
    }
}

fn polish_segment(state: &mut State, command: &Value) -> Result<(), String> {
    let item = session_mut(state, string(command, "sessionId")?)?;
    let segment = item
        .segments
        .iter_mut()
        .find(|segment| segment.id == string(command, "segmentId").unwrap_or_default())
        .ok_or("SEGMENT_NOT_FOUND")?;
    let expected_machine = uint(command, "expectedMachineRevision")?;
    let expected_user = uint(command, "expectedUserSeq")?;
    let source = string(command, "sourceText")?;
    let polished = string(command, "text")?.trim();
    if !segment.final_ {
        return Err("SEGMENT_NOT_FINAL".into());
    }
    if segment.machine_revision != expected_machine
        || segment.user_seq != expected_user
        || segment.display_text != source
    {
        return Err("POLISH_VERSION_CONFLICT".into());
    }
    if polished.is_empty() || polished.len() > MAX_TEXT_BYTES * 2 {
        return Err("INVALID_POLISH".into());
    }
    if !apply_polished_text(segment, source, polished)? {
        return Ok(());
    }
    bump(item);
    Ok(())
}

fn apply_polished_text(
    segment: &mut Segment,
    source: &str,
    polished: &str,
) -> Result<bool, String> {
    if polished == source.trim() {
        return Ok(false);
    }
    let (base_machine_text, base_machine_revision) = if segment.pending_machine.is_some() {
        segment
            .correction()
            .map(|correction| {
                (
                    correction.base_machine_text.clone(),
                    correction.base_machine_revision,
                )
            })
            .unwrap_or_else(|| (segment.machine_text.clone(), segment.machine_revision))
    } else {
        (segment.machine_text.clone(), segment.machine_revision)
    };
    let keep = usize::try_from(segment.history_index + 1).unwrap_or(0);
    segment.history.truncate(keep);
    segment.user_seq = segment.user_seq.checked_add(1).ok_or("USER_SEQ_OVERFLOW")?;
    segment.history.push(Some(Correction {
        base_machine_text,
        base_machine_revision,
        text: polished.to_owned(),
        user_seq: segment.user_seq,
        automatic: true,
    }));
    segment.history_index = segment.history.len() as i64 - 1;
    segment.refresh_projection();
    Ok(true)
}

struct PolishSource {
    id: String,
    expected_machine: u64,
    expected_user: u64,
    text: String,
}

struct PolishOutput {
    source_start: usize,
    source_end: usize,
    text: String,
}

fn polish_segments(state: &mut State, command: &Value) -> Result<(), String> {
    let source_values = command
        .get("sources")
        .and_then(Value::as_array)
        .ok_or("INVALID_POLISH")?;
    let output_values = command
        .get("segments")
        .and_then(Value::as_array)
        .ok_or("INVALID_POLISH")?;
    if source_values.is_empty() || source_values.len() > 8 || output_values.is_empty() {
        return Err("INVALID_POLISH".into());
    }
    let mut source_ids = HashSet::new();
    let mut sources = Vec::with_capacity(source_values.len());
    for value in source_values {
        let id = string(value, "id")?.to_owned();
        let text = string(value, "sourceText")?.to_owned();
        if !safe_id(&id)
            || !source_ids.insert(id.clone())
            || text.trim().is_empty()
            || text.len() > MAX_TEXT_BYTES * 2
        {
            return Err("INVALID_POLISH".into());
        }
        sources.push(PolishSource {
            id,
            expected_machine: uint(value, "expectedMachineRevision")?,
            expected_user: uint(value, "expectedUserSeq")?,
            text,
        });
    }
    let mut outputs = Vec::with_capacity(output_values.len());
    let mut cursor = 0usize;
    for value in output_values {
        let source_start =
            usize::try_from(uint(value, "sourceStart")?).map_err(|_| "INVALID_POLISH_MAPPING")?;
        let source_end =
            usize::try_from(uint(value, "sourceEnd")?).map_err(|_| "INVALID_POLISH_MAPPING")?;
        let text = string(value, "text")?.trim().to_owned();
        if source_start != cursor
            || source_end <= source_start
            || source_end > sources.len()
            || text.is_empty()
            || text.len() > MAX_TEXT_BYTES * 2
        {
            return Err("INVALID_POLISH_MAPPING".into());
        }
        cursor = source_end;
        outputs.push(PolishOutput {
            source_start,
            source_end,
            text,
        });
    }
    if cursor != sources.len() {
        return Err("INVALID_POLISH_MAPPING".into());
    }

    let item = session_mut(state, string(command, "sessionId")?)?;
    let positions: Vec<usize> = sources
        .iter()
        .map(|source| {
            item.segments
                .iter()
                .position(|segment| segment.id == source.id)
                .ok_or("POLISH_VERSION_CONFLICT")
        })
        .collect::<Result<_, _>>()?;
    if positions
        .windows(2)
        .any(|pair| pair[1] != pair[0].saturating_add(1))
    {
        return Err("POLISH_VERSION_CONFLICT".into());
    }
    let current: Vec<Segment> = positions
        .iter()
        .map(|position| item.segments[*position].clone())
        .collect();
    let run_id = &current[0].run_id;
    for (segment, source) in current.iter().zip(&sources) {
        if !segment.final_
            || &segment.run_id != run_id
            || segment.machine_revision != source.expected_machine
            || segment.user_seq != source.expected_user
            || segment.display_text != source.text
        {
            return Err("POLISH_VERSION_CONFLICT".into());
        }
    }
    if current.windows(2).any(|pair| {
        pair[1].start_sample < pair[0].start_sample || pair[1].end_sample < pair[0].end_sample
    }) {
        return Err("POLISH_VERSION_CONFLICT".into());
    }
    let open_drafts: HashSet<&str> = item
        .drafts
        .iter()
        .filter(|draft| draft.state == "open")
        .map(|draft| draft.segment_id.as_str())
        .collect();
    for output in &outputs {
        let group = &current[output.source_start..output.source_end];
        if group.iter().any(|segment| {
            segment.pending_machine.is_some()
                || open_drafts.contains(segment.id.as_str())
                || segment.history_index != segment.history.len() as i64 - 1
        }) {
            return Err(if group.len() > 1 {
                "POLISH_MERGE_CONFLICT"
            } else {
                "POLISH_VERSION_CONFLICT"
            }
            .into());
        }
        if group.len() > 1 && group.iter().any(Segment::has_human_history) {
            return Err("POLISH_MERGE_CONFLICT".into());
        }
    }

    let mut replacements = Vec::with_capacity(outputs.len());
    let mut changed = false;
    for output in &outputs {
        let group = &current[output.source_start..output.source_end];
        let mut survivor = group[0].clone();
        if group.len() == 1 {
            changed |= apply_polished_text(
                &mut survivor,
                &sources[output.source_start].text,
                &output.text,
            )?;
        } else {
            changed = true;
            survivor.end_sample = group.last().unwrap().end_sample;
            let combined_source = sources[output.source_start..output.source_end]
                .iter()
                .map(|source| source.text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            if combined_source.len() > MAX_TEXT_BYTES {
                return Err("INVALID_POLISH".into());
            }
            survivor.machine_text = combined_source.clone();
            survivor.machine_revision = group
                .iter()
                .map(|segment| segment.machine_revision)
                .max()
                .unwrap_or(survivor.machine_revision);
            survivor.worker_epoch = group
                .iter()
                .map(|segment| segment.worker_epoch)
                .max()
                .unwrap_or(survivor.worker_epoch);
            survivor.machine_history = vec![MachineHypothesis {
                revision: survivor.machine_revision,
                worker_epoch: survivor.worker_epoch,
                text: survivor.machine_text.clone(),
                final_: true,
            }];
            survivor.history.clear();
            survivor.history_index = -1;
            survivor.user_seq = 0;
            survivor.pending_machine = None;
            survivor.corrected = false;
            survivor.display_text = survivor.machine_text.clone();
            apply_polished_text(&mut survivor, &combined_source, &output.text)?;
        }
        replacements.push(survivor);
    }
    if !changed {
        return Ok(());
    }
    let survivor_for: std::collections::HashMap<String, String> = outputs
        .iter()
        .flat_map(|output| {
            let survivor = current[output.source_start].id.clone();
            current[output.source_start + 1..output.source_end]
                .iter()
                .map(move |segment| (segment.id.clone(), survivor.clone()))
        })
        .collect();
    for note in &mut item.notes {
        if let Some(survivor) = survivor_for.get(&note.segment_id) {
            note.segment_id = survivor.clone();
        }
    }
    for draft in &mut item.drafts {
        if let Some(survivor) = survivor_for.get(&draft.segment_id) {
            draft.segment_id = survivor.clone();
        }
    }
    let first = positions[0];
    let last = positions[positions.len() - 1];
    item.segments.splice(first..=last, replacements);
    bump(item);
    Ok(())
}

fn demo_tick(state: &mut State, command: &Value) -> Result<(), String> {
    let worker_epoch = state.worker_epoch;
    let item = session_mut(state, string(command, "sessionId")?)?;
    if item.mode != "demo" {
        return Err("DEMO_ONLY".into());
    }
    let run_id = "demo-run".to_string();
    if item.runs.is_empty() {
        item.runs.push(Run {
            id: run_id.clone(),
            source: "demo".into(),
            engine: "qwen".into(),
            started_at: now_ms(),
            ended_at: None,
            samples: 0,
            offset_ms: 0,
            state: "recording".into(),
        });
        item.recording_state = "recording".into();
        item.inference_state = "running".into();
    }
    let lines = [
        "Economics studies how people make choices when resources are limited.",
        "Opportunity cost is the value of the best alternative given up when making a choice.",
        "The next example connects prices and demand with everyday consumer choices.",
    ];
    let index = item.segments.len();
    if let Some(text) = lines.get(index) {
        let start = index as u64 * 160_000;
        let end = start + 160_000;
        item.segments.push(Segment {
            id: format!("demo-segment-{index}"),
            run_id: run_id.clone(),
            start_sample: start,
            end_sample: end,
            machine_text: (*text).into(),
            machine_revision: 1,
            final_: true,
            worker_epoch,
            user_seq: 0,
            display_text: (*text).into(),
            pending_machine: None,
            corrected: false,
            history: Vec::new(),
            history_index: -1,
            machine_history: vec![MachineHypothesis {
                revision: 1,
                worker_epoch,
                text: (*text).into(),
                final_: true,
            }],
        });
        item.runs[0].samples = end;
    } else {
        item.recording_state = "stopped".into();
        item.inference_state = "idle".into();
        item.runs[0].ended_at = Some(now_ms());
        item.runs[0].state = "complete".into();
    }
    bump(item);
    Ok(())
}

pub fn validate_state(state: &State) -> Result<(), String> {
    if state.schema_version != 1 {
        return Err("UNSUPPORTED_SCHEMA".into());
    }
    if state.sessions.len() > 10_000 || state.projects.len() > 10_000 {
        return Err("STATE_TOO_LARGE".into());
    }
    if [
        &state.settings.engine,
        &state.settings.executable,
        &state.settings.model_path,
        &state.settings.mmproj_path,
        &state.settings.language,
    ]
    .into_iter()
    .any(|value| value.len() > 64 * 1024)
    {
        return Err("INVALID_SETTINGS".into());
    }
    validate_vocabulary(&state.settings.custom_vocabulary)?;
    let mut project_ids = HashSet::new();
    for project in &state.projects {
        if !project_ids.insert(project.id.as_str()) {
            return Err("DUPLICATE_PROJECT".into());
        }
        if !safe_id(&project.id)
            || project.title.trim().is_empty()
            || project.title.len() > MAX_TITLE_BYTES
        {
            return Err("INVALID_PROJECT".into());
        }
        validate_vocabulary(&project.custom_vocabulary)?;
    }
    let mut ids = HashSet::new();
    for item in &state.sessions {
        if !ids.insert(item.id.as_str()) {
            return Err("DUPLICATE_SESSION".into());
        }
        validate_session(item)?;
        validate_vocabulary(&item.custom_vocabulary)?;
        if item
            .project_id
            .as_deref()
            .is_some_and(|id| !project_ids.contains(id))
        {
            return Err("INVALID_PROJECT_REFERENCE".into());
        }
    }
    if let Some(selected) = &state.selected_session_id {
        if !state.sessions.iter().any(|item| &item.id == selected) {
            return Err("INVALID_SELECTION".into());
        }
    }
    Ok(())
}

pub fn validate_session(item: &Session) -> Result<(), String> {
    validate_vocabulary(&item.custom_vocabulary)?;
    if !safe_id(&item.id)
        || item.title.trim().is_empty()
        || item.title.len() > MAX_TITLE_BYTES
        || !matches!(item.mode.as_str(), "live" | "demo")
        || item.runs.len() > MAX_ITEMS
        || item.segments.len() > MAX_ITEMS
        || item.notes.len() > MAX_ITEMS
        || item.drafts.len() > MAX_ITEMS
        || item.gaps.len() > MAX_ITEMS
        || item.recording_state.len() > 128
        || item.inference_state.len() > 128
        || item
            .error
            .as_ref()
            .is_some_and(|error| error.len() > MAX_TEXT_BYTES)
    {
        return Err("INVALID_SESSION".into());
    }
    let run_ids: HashSet<&str> = item.runs.iter().map(|run| run.id.as_str()).collect();
    if run_ids.len() != item.runs.len()
        || run_ids.iter().any(|id| !safe_id(id))
        || item.runs.iter().any(|run| {
            run.source.len() > MAX_TITLE_BYTES
                || !matches!(
                    run.engine.as_str(),
                    "qwen" | "qwen3-asr" | "whisper" | "soniox"
                )
                || run.state.len() > 128
                || run.ended_at.is_some_and(|ended| ended < run.started_at)
        })
    {
        return Err("INVALID_RUN".into());
    }
    let mut segment_ids = HashSet::new();
    for segment in &item.segments {
        if !segment_ids.insert(segment.id.as_str())
            || !safe_id(&segment.id)
            || !safe_id(&segment.run_id)
            || segment.end_sample < segment.start_sample
            || segment.history_index < -1
            || segment.history_index >= segment.history.len() as i64
            || segment.machine_text.len() > MAX_TEXT_BYTES
            || segment.display_text.len() > MAX_TEXT_BYTES * 2
            || segment
                .pending_machine
                .as_ref()
                .is_some_and(|text| text.len() > MAX_TEXT_BYTES)
            || segment.history.len() > 10_000
            || segment.machine_history.len() > 10_000
            || segment.history.iter().flatten().any(|correction| {
                correction.base_machine_text.len() > MAX_TEXT_BYTES
                    || correction.text.len() > MAX_TEXT_BYTES * 2
            })
            || segment
                .machine_history
                .iter()
                .any(|hypothesis| hypothesis.text.len() > MAX_TEXT_BYTES)
        {
            return Err("INVALID_SEGMENT".into());
        }
        if !run_ids.is_empty() && !run_ids.contains(segment.run_id.as_str()) {
            return Err("INVALID_SEGMENT_RUN".into());
        }
        if item
            .runs
            .iter()
            .find(|run| run.id == segment.run_id)
            .is_some_and(|run| segment.end_sample > run.samples)
        {
            return Err("INVALID_SEGMENT_RANGE".into());
        }
        if segment
            .machine_history
            .windows(2)
            .any(|window| window[0].revision >= window[1].revision)
        {
            return Err("INVALID_MACHINE_HISTORY".into());
        }
        if segment
            .machine_history
            .iter()
            .take(segment.machine_history.len().saturating_sub(1))
            .any(|hypothesis| hypothesis.final_)
        {
            return Err("INVALID_MACHINE_HISTORY".into());
        }
        let last = segment
            .machine_history
            .last()
            .ok_or("MISSING_MACHINE_HISTORY")?;
        if last.revision != segment.machine_revision
            || last.text != segment.machine_text
            || last.final_ != segment.final_
            || last.worker_epoch != segment.worker_epoch
            || segment.history.iter().flatten().any(|correction| {
                correction.base_machine_revision > segment.machine_revision
                    || correction.user_seq > segment.user_seq
            })
        {
            return Err("INVALID_SEGMENT_HISTORY".into());
        }
        let projection = segment.projection();
        if projection.text != segment.display_text
            || projection.pending_machine != segment.pending_machine
            || segment.corrected != segment.correction().is_some()
        {
            return Err("INVALID_SEGMENT_PROJECTION".into());
        }
    }
    let mut note_ids = HashSet::new();
    for note in &item.notes {
        let anchor = item
            .segments
            .iter()
            .find(|segment| segment.id == note.segment_id);
        if !note_ids.insert(note.id.as_str())
            || !safe_id(&note.id)
            || anchor.is_none()
            || anchor.is_some_and(|segment| {
                note.run_id != segment.run_id
                    || note.sample < segment.start_sample
                    || note.sample > segment.end_sample
            })
            || !matches!(note.kind.as_str(), "note" | "example" | "formula" | "image")
            || note.text.len() > MAX_TEXT_BYTES
            || note
                .source_label
                .as_ref()
                .is_some_and(|label| label.len() > MAX_TITLE_BYTES)
            || (note.kind == "image"
                && note
                    .image_data
                    .as_deref()
                    .is_none_or(|data| validate_image_data(data).is_err()))
            || (note.kind != "image" && note.image_data.is_some())
        {
            return Err("INVALID_NOTE".into());
        }
    }
    let mut draft_ids = HashSet::new();
    for draft in &item.drafts {
        if !draft_ids.insert(draft.id.as_str())
            || !safe_id(&draft.id)
            || !segment_ids.contains(draft.segment_id.as_str())
            || draft.base_machine_text.len() > MAX_TEXT_BYTES
            || draft.snapshot_text.len() > MAX_TEXT_BYTES * 2
            || draft.text.len() > MAX_TEXT_BYTES * 2
            || !matches!(
                draft.state.as_str(),
                "open" | "closed" | "committed" | "discarded"
            )
        {
            return Err("INVALID_DRAFT".into());
        }
    }
    for gap in &item.gaps {
        if !safe_id(&gap.run_id)
            || gap.end_sample < gap.start_sample
            || gap.reason.len() > MAX_TITLE_BYTES
            || (!run_ids.is_empty() && !run_ids.contains(gap.run_id.as_str()))
        {
            return Err("INVALID_GAP".into());
        }
    }
    Ok(())
}

fn validate_image_data(value: &str) -> Result<(), String> {
    let payload = [
        "data:image/png;base64,",
        "data:image/jpeg;base64,",
        "data:image/webp;base64,",
    ]
    .into_iter()
    .find_map(|prefix| value.strip_prefix(prefix))
    .ok_or("INVALID_IMAGE_DATA")?;
    if payload.len() > (MAX_IMAGE_BYTES * 4 / 3) + 8 {
        return Err("IMAGE_TOO_LARGE".into());
    }
    let decoded = STANDARD.decode(payload).map_err(|_| "INVALID_IMAGE_DATA")?;
    if decoded.is_empty() || decoded.len() > MAX_IMAGE_BYTES {
        return Err("IMAGE_TOO_LARGE".into());
    }
    let valid_signature = if value.starts_with("data:image/png") {
        decoded.starts_with(b"\x89PNG\r\n\x1a\n")
    } else if value.starts_with("data:image/jpeg") {
        decoded.starts_with(&[0xff, 0xd8, 0xff])
    } else {
        decoded.starts_with(b"RIFF") && decoded.get(8..12) == Some(b"WEBP")
    };
    if !valid_signature {
        return Err("INVALID_IMAGE_DATA".into());
    }
    Ok(())
}

pub fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn session<'a>(state: &'a State, id: &str) -> Result<&'a Session, String> {
    state
        .sessions
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| "SESSION_NOT_FOUND".into())
}

fn project<'a>(state: &'a State, id: &str) -> Result<&'a Project, String> {
    state
        .projects
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| "PROJECT_NOT_FOUND".into())
}

fn session_mut<'a>(state: &'a mut State, id: &str) -> Result<&'a mut Session, String> {
    state
        .sessions
        .iter_mut()
        .find(|item| item.id == id)
        .ok_or_else(|| "SESSION_NOT_FOUND".into())
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("INVALID_{key}"))
}

fn nullable_string<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match value.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        _ => Err(format!("INVALID_{key}")),
    }
}

fn optional_nullable_string<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        _ => Err(format!("INVALID_{key}")),
    }
}

fn valid_title(value: &Value) -> Result<&str, String> {
    let title = string(value, "title")?.trim();
    if title.is_empty() || title.len() > MAX_TITLE_BYTES {
        return Err("INVALID_TITLE".into());
    }
    Ok(title)
}

fn uint(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("INVALID_{key}"))
}

fn boolean(value: &Value, key: &str) -> Result<bool, String> {
    value
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("INVALID_{key}"))
}

fn bump(item: &mut Session) {
    item.operation_seq += 1;
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup() -> (State, String) {
        let mut state = State::default();
        apply_command(
            &mut state,
            &json!({"type":"createSession","title":"Motion","mode":"live"}),
        )
        .unwrap();
        let session = state.selected_session_id.clone().unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":160000,"text":"According to Talmey, Path is encoded","revision":1,"workerEpoch":0,"final":false})).unwrap();
        (state, session)
    }

    #[test]
    fn project_commands_validate_references_and_preserve_session_identity() {
        let mut state = State::default();
        apply_command(
            &mut state,
            &json!({"type":"createProject","title":"  Economics  "}),
        )
        .unwrap();
        let project_id = state.projects[0].id.clone();
        apply_command(
            &mut state,
            &json!({"type":"renameProject","projectId":project_id,"title":"Microeconomics"}),
        )
        .unwrap();
        apply_command(
            &mut state,
            &json!({"type":"createSession","title":"Markets","mode":"live","projectId":project_id}),
        )
        .unwrap();
        let session_id = state.sessions[0].id.clone();
        state.sessions[0].runs.push(Run {
            id: "audio-run".into(),
            source: "microphone".into(),
            engine: "qwen".into(),
            started_at: 1,
            ended_at: Some(2),
            samples: 0,
            offset_ms: 0,
            state: "complete".into(),
        });

        apply_command(
            &mut state,
            &json!({"type":"moveSession","sessionId":session_id,"projectId":null}),
        )
        .unwrap();
        assert_eq!(state.projects[0].title, "Microeconomics");
        assert_eq!(state.sessions[0].id, session_id);
        assert_eq!(state.sessions[0].runs[0].id, "audio-run");
        assert_eq!(state.sessions[0].project_id, None);

        let before = state.clone();
        assert_eq!(
            apply_command(
                &mut state,
                &json!({"type":"createSession","title":"Invalid","mode":"live","projectId":"missing"}),
            )
            .unwrap_err(),
            "PROJECT_NOT_FOUND"
        );
        assert_eq!(state, before);
        assert_eq!(
            apply_command(
                &mut state,
                &json!({"type":"moveSession","sessionId":session_id,"projectId":"missing"}),
            )
            .unwrap_err(),
            "PROJECT_NOT_FOUND"
        );
        assert_eq!(state, before);
    }

    #[test]
    fn state_rejects_dangling_project_membership() {
        let mut state = State::default();
        let mut session = Session::new("Markets".into(), "live".into());
        session.project_id = Some("missing".into());
        state.sessions.push(session);
        assert_eq!(
            validate_state(&state).unwrap_err(),
            "INVALID_PROJECT_REFERENCE"
        );
    }

    fn edit(state: &mut State, session: &str, text: &str) {
        apply_command(
            state,
            &json!({"type":"beginEdit","sessionId":session,"segmentId":"s1"}),
        )
        .unwrap();
        let draft = state.sessions[0].drafts.last().unwrap().clone();
        apply_command(state, &json!({"type":"commitEdit","sessionId":session,"draftId":draft.id,"text":text,"expectedUserSeq":draft.expected_user_seq})).unwrap();
    }

    #[test]
    fn safe_suffix_and_later_conflict_keep_complete_human_text() {
        let (mut state, session) = setup();
        edit(&mut state, &session, "According to Talmy, Path is encoded");
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":190000,"text":"According to Talmey, Path is encoded in the verb.","revision":2,"workerEpoch":0,"final":false})).unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "According to Talmy, Path is encoded in the verb."
        );
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":200000,"text":"According to Chomsky, Path is encoded in the verb. More follows.","revision":3,"workerEpoch":0,"final":false})).unwrap();
        let segment = &state.sessions[0].segments[0];
        assert_eq!(
            segment.display_text,
            "According to Talmy, Path is encoded in the verb."
        );
        assert_eq!(
            segment.pending_machine.as_deref(),
            Some("According to Chomsky, Path is encoded in the verb. More follows.")
        );
    }

    #[test]
    fn machine_agreement_does_not_remove_human_protection() {
        let (mut state, session) = setup();
        edit(&mut state, &session, "According to Talmy, Path is encoded");
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":160000,"text":"According to Talmy, Path is encoded","revision":2,"workerEpoch":0,"final":false})).unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":160000,"text":"According to Chomsky, Path is encoded","revision":3,"workerEpoch":0,"final":false})).unwrap();
        let segment = &state.sessions[0].segments[0];
        assert!(segment.corrected);
        assert!(segment.display_text.contains("Talmy"));
        assert!(segment.pending_machine.is_some());
    }

    #[test]
    fn final_epoch_revision_and_range_are_guarded() {
        let (mut state, session) = setup();
        state.worker_epoch = 1;
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":160001,"text":"old","revision":2,"workerEpoch":0,"final":false})).unwrap();
        assert_eq!(state.sessions[0].segments[0].machine_revision, 1);
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":170000,"text":"final","revision":2,"workerEpoch":1,"final":true})).unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":180000,"text":"late","revision":3,"workerEpoch":1,"final":false})).unwrap();
        assert_eq!(state.sessions[0].segments[0].machine_text, "final");
        let error = apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":"r1","startSample":100,"endSample":99,"text":"bad","revision":1,"workerEpoch":1,"final":false})).unwrap_err();
        assert_eq!(error, "INVALID_HYPOTHESIS");
    }

    #[test]
    fn final_sentence_split_is_atomic_and_preserves_first_segment_edit() {
        let (mut state, session) = setup();
        edit(&mut state, &session, "According to Talmy, Path is encoded");
        let split = json!({
            "type":"machineSegments",
            "sessionId":session,
            "runId":"r1",
            "workerEpoch":0,
            "segments":[
                {"id":"s1","startSample":0,"endSample":70_000,"text":"According to Talmey, Path is encoded.","revision":2},
                {"id":"s1_sentence_2","startSample":70_000,"endSample":160_000,"text":"The second sentence follows.","revision":1}
            ]
        });
        apply_command(&mut state, &split).unwrap();

        let first = &state.sessions[0].segments[0];
        assert_eq!(first.id, "s1");
        assert_eq!(first.end_sample, 70_000);
        assert!(first.final_);
        assert!(first.corrected);
        assert!(first.display_text.contains("Talmy"));
        let second = &state.sessions[0].segments[1];
        assert_eq!(second.start_sample, first.end_sample);
        assert_eq!(second.end_sample, 160_000);
        assert!(second.final_);
        validate_state(&state).unwrap();

        let applied = state.clone();
        apply_command(&mut state, &split).unwrap();
        assert_eq!(state, applied);
    }

    #[test]
    fn confirmed_cloud_token_range_can_replace_a_longer_provisional_range() {
        let (mut state, session) = setup();
        let ordinary = json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"First sentence.","revision":2,"workerEpoch":0,"final":true});
        assert_eq!(
            apply_command(&mut state, &ordinary).unwrap_err(),
            "AUDIO_RANGE_SHRANK"
        );
        let cloud_final = json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"First sentence.","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true});
        apply_command(&mut state, &cloud_final).unwrap();
        assert_eq!(state.sessions[0].segments[0].end_sample, 80_000);
        assert!(state.sessions[0].segments[0].final_);
    }

    #[test]
    fn undo_and_redo_move_only_human_history() {
        let (mut state, session) = setup();
        edit(&mut state, &session, "According to Talmy, Path is encoded");
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":190000,"text":"According to Talmey, Path is encoded in the verb.","revision":2,"workerEpoch":0,"final":false})).unwrap();
        let seq = state.sessions[0].segments[0].user_seq;
        apply_command(
            &mut state,
            &json!({"type":"undo","sessionId":session,"segmentId":"s1","expectedUserSeq":seq}),
        )
        .unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "According to Talmey, Path is encoded in the verb."
        );
        let seq = state.sessions[0].segments[0].user_seq;
        apply_command(
            &mut state,
            &json!({"type":"redo","sessionId":session,"segmentId":"s1","expectedUserSeq":seq}),
        )
        .unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "According to Talmy, Path is encoded in the verb."
        );
    }

    #[test]
    fn serialized_contract_uses_camel_case_and_final() {
        let (state, _) = setup();
        let value = serde_json::to_value(state).unwrap();
        let segment = &value["sessions"][0]["segments"][0];
        assert!(segment.get("machineText").is_some());
        assert!(segment.get("startSample").is_some());
        assert!(segment.get("final").is_some());
        assert!(segment.get("final_").is_none());
    }

    #[test]
    fn note_images_and_audio_anchors_are_validated() {
        let (mut state, session) = setup();
        let bad = apply_command(&mut state, &json!({"type":"addNote","sessionId":session,"segmentId":"s1","kind":"image","text":"diagram","imageData":"data:text/html;base64,PGgxPmJhZDwvaDE+"})).unwrap_err();
        assert_eq!(bad, "INVALID_NOTE");
        assert!(state.sessions[0].notes.is_empty());

        let png = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(b"\x89PNG\r\n\x1a\nminimal")
        );
        apply_command(&mut state, &json!({"type":"addNote","sessionId":session,"segmentId":"s1","kind":"image","text":"diagram","imageData":png})).unwrap();
        assert_eq!(state.sessions[0].notes[0].sample, 0);
        state.sessions[0].notes[0].sample = 160_001;
        assert_eq!(
            validate_session(&state.sessions[0]).unwrap_err(),
            "INVALID_NOTE"
        );
    }

    #[test]
    fn changed_settings_advance_epoch_and_reject_old_worker() {
        let (mut state, session) = setup();
        let mut settings = state.settings.clone();
        settings.language = "en".into();
        apply_command(&mut state, &json!({"type":"settings","settings":settings})).unwrap();
        assert_eq!(state.worker_epoch, 1);
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":180000,"text":"stale result","revision":2,"workerEpoch":0,"final":false})).unwrap();
        assert_eq!(state.sessions[0].segments[0].machine_revision, 1);

        let epoch = state.worker_epoch;
        let settings = state.settings.clone();
        apply_command(&mut state, &json!({"type":"settings","settings":settings})).unwrap();
        assert_eq!(state.worker_epoch, epoch);
    }

    #[test]
    fn legacy_settings_default_to_system_appearance() {
        let settings: Settings = serde_json::from_value(json!({
            "engine":"qwen",
            "executable":"",
            "modelPath":"",
            "mmprojPath":"",
            "language":"en",
            "cloudConsent":false
        }))
        .unwrap();
        assert_eq!(settings.theme, "system");
        assert!(settings.custom_vocabulary.is_empty());
        assert!(!settings.auto_polish);
    }

    #[test]
    fn legacy_corrections_default_to_human_origin() {
        let correction: Correction = serde_json::from_value(json!({
            "baseMachineText":"machine",
            "baseMachineRevision":1,
            "text":"human edit",
            "userSeq":1
        }))
        .unwrap();
        assert!(!correction.automatic);
    }

    #[test]
    fn polish_preserves_machine_text_and_builds_on_existing_human_edits() {
        let (mut state, session) = setup();
        apply_command(
            &mut state,
            &json!({"type":"beginEdit","sessionId":session,"segmentId":"s1"}),
        )
        .unwrap();
        let draft = state.sessions[0].drafts[0].clone();
        apply_command(
            &mut state,
            &json!({"type":"commitEdit","sessionId":session,"draftId":draft.id,"expectedUserSeq":draft.expected_user_seq,"text":"According to Talmy, um, Path is encoded"}),
        )
        .unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":180000,"text":"According to Talmey. Um, Path is encoded in the verb.","revision":2,"workerEpoch":0,"final":true})).unwrap();
        let before = state.sessions[0].segments[0].clone();
        apply_command(
            &mut state,
            &json!({
                "type":"polishSegment",
                "sessionId":session,
                "segmentId":"s1",
                "expectedMachineRevision":before.machine_revision,
                "expectedUserSeq":before.user_seq,
                "sourceText":before.display_text,
                "text":"According to Talmy, Path is encoded in the verb."
            }),
        )
        .unwrap();
        let segment = &state.sessions[0].segments[0];
        assert_eq!(
            segment.machine_text,
            "According to Talmey. Um, Path is encoded in the verb."
        );
        assert_eq!(
            segment.display_text,
            "According to Talmy, Path is encoded in the verb."
        );
        assert_eq!(segment.user_seq, before.user_seq + 1);
        assert_eq!(segment.history.len(), before.history.len() + 1);

        let polished_seq = segment.user_seq;
        apply_command(
            &mut state,
            &json!({"type":"undo","sessionId":session,"segmentId":"s1","expectedUserSeq":polished_seq}),
        )
        .unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            before.display_text
        );
        let undone_seq = state.sessions[0].segments[0].user_seq;
        apply_command(
            &mut state,
            &json!({"type":"redo","sessionId":session,"segmentId":"s1","expectedUserSeq":undone_seq}),
        )
        .unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "According to Talmy, Path is encoded in the verb."
        );
    }

    #[test]
    fn polish_rejects_unfinalized_or_stale_segments_without_overwriting_edits() {
        let (mut state, session) = setup();
        let before = state.clone();
        assert_eq!(
            apply_command(
                &mut state,
                &json!({"type":"polishSegment","sessionId":session,"segmentId":"s1","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"According to Talmey, Path is encoded","text":"According to Talmy, Path is encoded."}),
            )
            .unwrap_err(),
            "SEGMENT_NOT_FINAL"
        );
        assert_eq!(state, before);

        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":180000,"text":"According to Talmey, Path is encoded in the verb.","revision":2,"workerEpoch":0,"final":true})).unwrap();
        let source = state.sessions[0].segments[0].display_text.clone();
        apply_command(
            &mut state,
            &json!({"type":"beginEdit","sessionId":session,"segmentId":"s1"}),
        )
        .unwrap();
        let draft = state.sessions[0].drafts.last().unwrap().clone();
        apply_command(
            &mut state,
            &json!({"type":"commitEdit","sessionId":session,"draftId":draft.id,"expectedUserSeq":draft.expected_user_seq,"text":"My protected edit."}),
        )
        .unwrap();
        assert_eq!(
            apply_command(
                &mut state,
                &json!({"type":"polishSegment","sessionId":session,"segmentId":"s1","expectedMachineRevision":2,"expectedUserSeq":0,"sourceText":source,"text":"Stale background result."}),
            )
            .unwrap_err(),
            "POLISH_VERSION_CONFLICT"
        );
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "My protected edit."
        );
    }

    #[test]
    fn polish_preserves_a_pending_machine_conflict() {
        let (mut state, session) = setup();
        apply_command(
            &mut state,
            &json!({"type":"beginEdit","sessionId":session,"segmentId":"s1"}),
        )
        .unwrap();
        let draft = state.sessions[0].drafts[0].clone();
        apply_command(
            &mut state,
            &json!({"type":"commitEdit","sessionId":session,"draftId":draft.id,"expectedUserSeq":draft.expected_user_seq,"text":"A protected human interpretation."}),
        )
        .unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":180000,"text":"A substantially different machine proposal with new claims.","revision":2,"workerEpoch":0,"final":true})).unwrap();
        let before = state.sessions[0].segments[0].clone();
        assert!(before.pending_machine.is_some());
        apply_command(
            &mut state,
            &json!({
                "type":"polishSegment",
                "sessionId":session,
                "segmentId":"s1",
                "expectedMachineRevision":before.machine_revision,
                "expectedUserSeq":before.user_seq,
                "sourceText":before.display_text,
                "text":"A protected human interpretation"
            }),
        )
        .unwrap();
        let segment = &state.sessions[0].segments[0];
        assert_eq!(segment.display_text, "A protected human interpretation");
        assert_eq!(segment.pending_machine, before.pending_machine);
        assert_eq!(segment.machine_text, before.machine_text);
        assert_eq!(segment.history.len(), before.history.len() + 1);
    }

    #[test]
    fn multi_segment_polish_joins_audio_ranges_and_remaps_notes_atomically() {
        let (mut state, session) = setup();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"The demand curve.","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":"r1","startSample":80_000,"endSample":160_000,"text":"Shifts when income changes.","revision":1,"workerEpoch":0,"final":true})).unwrap();
        apply_command(&mut state, &json!({"type":"addNote","sessionId":session,"segmentId":"s2","kind":"note","text":"income effect","sourceLabel":null,"imageData":null})).unwrap();
        let note_sample = state.sessions[0].notes[0].sample;
        let command = json!({
            "type":"polishSegments",
            "sessionId":session,
            "sources":[
                {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":0,"sourceText":"The demand curve."},
                {"id":"s2","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"Shifts when income changes."}
            ],
            "segments":[
                {"sourceStart":0,"sourceEnd":2,"text":"The demand curve shifts when income changes."}
            ]
        });
        apply_command(&mut state, &command).unwrap();

        assert_eq!(state.sessions[0].segments.len(), 1);
        let segment = &state.sessions[0].segments[0];
        assert_eq!(segment.id, "s1");
        assert_eq!(segment.start_sample, 0);
        assert_eq!(segment.end_sample, 160_000);
        assert_eq!(
            segment.display_text,
            "The demand curve shifts when income changes."
        );
        assert_eq!(state.sessions[0].notes[0].segment_id, "s1");
        assert_eq!(state.sessions[0].notes[0].sample, note_sample);

        let seq = segment.user_seq;
        apply_command(
            &mut state,
            &json!({"type":"undo","sessionId":session,"segmentId":"s1","expectedUserSeq":seq}),
        )
        .unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "The demand curve. Shifts when income changes."
        );
        assert_eq!(state.sessions[0].segments[0].end_sample, 160_000);
        validate_state(&state).unwrap();
        let restored: State =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(restored, state);
    }

    #[test]
    fn multi_segment_polish_rejects_structural_merge_after_human_edit_but_allows_atomic_one_to_one_polish(
    ) {
        let (mut state, session) = setup();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"A protected first fragment","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":"r1","startSample":80_000,"endSample":160_000,"text":"and the second fragment.","revision":1,"workerEpoch":0,"final":true})).unwrap();
        edit(&mut state, &session, "A human-protected first fragment");
        let before = state.clone();
        let sources = json!([
            {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":1,"sourceText":"A human-protected first fragment"},
            {"id":"s2","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"and the second fragment."}
        ]);
        assert_eq!(
            apply_command(
                &mut state,
                &json!({"type":"polishSegments","sessionId":session,"sources":sources,"segments":[{"sourceStart":0,"sourceEnd":2,"text":"A human-protected first fragment and the second fragment."}]})
            )
            .unwrap_err(),
            "POLISH_MERGE_CONFLICT"
        );
        assert_eq!(state, before);

        apply_command(
            &mut state,
            &json!({"type":"polishSegments","sessionId":session,"sources":sources,"segments":[
                {"sourceStart":0,"sourceEnd":1,"text":"A human-protected first fragment."},
                {"sourceStart":1,"sourceEnd":2,"text":"And the second fragment."}
            ]}),
        )
        .unwrap();
        assert_eq!(state.sessions[0].segments.len(), 2);
        assert!(state.sessions[0].segments[0]
            .display_text
            .contains("human-protected"));
    }

    #[test]
    fn single_segment_background_polish_respects_open_drafts_and_undo() {
        let (mut open, session) = setup();
        apply_command(&mut open, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"An open draft sentence.","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
        apply_command(
            &mut open,
            &json!({"type":"beginEdit","sessionId":session,"segmentId":"s1"}),
        )
        .unwrap();
        let before = open.clone();
        assert_eq!(
            apply_command(
                &mut open,
                &json!({"type":"polishSegments","sessionId":session,"sources":[
                    {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":0,"sourceText":"An open draft sentence."}
                ],"segments":[{"sourceStart":0,"sourceEnd":1,"text":"An open draft sentence"}]})
            )
            .unwrap_err(),
            "POLISH_VERSION_CONFLICT"
        );
        assert_eq!(open, before);

        let (mut undone, session) = setup();
        apply_command(&mut undone, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"An automatic sentence.","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
        apply_command(
            &mut undone,
            &json!({"type":"polishSegments","sessionId":session,"sources":[
                {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":0,"sourceText":"An automatic sentence."}
            ],"segments":[{"sourceStart":0,"sourceEnd":1,"text":"An automatic sentence"}]}),
        )
        .unwrap();
        let polished_seq = undone.sessions[0].segments[0].user_seq;
        apply_command(
            &mut undone,
            &json!({"type":"undo","sessionId":session,"segmentId":"s1","expectedUserSeq":polished_seq}),
        )
        .unwrap();
        let segment = undone.sessions[0].segments[0].clone();
        let before = undone.clone();
        assert_eq!(
            apply_command(
                &mut undone,
                &json!({"type":"polishSegments","sessionId":session,"sources":[
                    {"id":"s1","expectedMachineRevision":segment.machine_revision,"expectedUserSeq":segment.user_seq,"sourceText":segment.display_text}
                ],"segments":[{"sourceStart":0,"sourceEnd":1,"text":"An automatic sentence"}]})
            )
            .unwrap_err(),
            "POLISH_VERSION_CONFLICT"
        );
        assert_eq!(undone, before);
    }

    #[test]
    fn multi_segment_polish_rolls_back_stale_open_pending_and_cross_run_merges() {
        fn pair(run_two: &str) -> (State, String) {
            let (mut state, session) = setup();
            apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"First fragment","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
            apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":run_two,"startSample":80_000,"endSample":160_000,"text":"second fragment.","revision":1,"workerEpoch":0,"final":true})).unwrap();
            (state, session)
        }
        fn merge(session: &str, first_revision: u64, first_user: u64, first_text: &str) -> Value {
            json!({"type":"polishSegments","sessionId":session,"sources":[
                {"id":"s1","expectedMachineRevision":first_revision,"expectedUserSeq":first_user,"sourceText":first_text},
                {"id":"s2","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"second fragment."}
            ],"segments":[{"sourceStart":0,"sourceEnd":2,"text":"First fragment second fragment."}]})
        }

        let (mut stale, session) = pair("r1");
        let before = stale.clone();
        assert_eq!(
            apply_command(&mut stale, &merge(&session, 99, 0, "First fragment")).unwrap_err(),
            "POLISH_VERSION_CONFLICT"
        );
        assert_eq!(stale, before);

        let (mut open, session) = pair("r1");
        apply_command(
            &mut open,
            &json!({"type":"beginEdit","sessionId":session,"segmentId":"s1"}),
        )
        .unwrap();
        let before = open.clone();
        assert_eq!(
            apply_command(&mut open, &merge(&session, 2, 0, "First fragment")).unwrap_err(),
            "POLISH_MERGE_CONFLICT"
        );
        assert_eq!(open, before);

        let (mut pending, session) = setup();
        edit(&mut pending, &session, "A protected interpretation");
        apply_command(&mut pending, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"A conflicting machine interpretation","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
        apply_command(&mut pending, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":"r1","startSample":80_000,"endSample":160_000,"text":"second fragment.","revision":1,"workerEpoch":0,"final":true})).unwrap();
        assert!(pending.sessions[0].segments[0].pending_machine.is_some());
        let before = pending.clone();
        assert_eq!(
            apply_command(
                &mut pending,
                &merge(&session, 2, 1, "A protected interpretation")
            )
            .unwrap_err(),
            "POLISH_MERGE_CONFLICT"
        );
        assert_eq!(pending, before);

        let (mut cross_run, session) = pair("r2");
        let before = cross_run.clone();
        assert_eq!(
            apply_command(&mut cross_run, &merge(&session, 2, 0, "First fragment")).unwrap_err(),
            "POLISH_VERSION_CONFLICT"
        );
        assert_eq!(cross_run, before);
    }

    #[test]
    fn automatically_polished_segment_can_later_merge_with_a_new_final_segment() {
        let (mut state, session) = setup();
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":80_000,"text":"The demand curve.","revision":2,"workerEpoch":0,"final":true,"allowFinalRangeShrink":true})).unwrap();
        apply_command(
            &mut state,
            &json!({"type":"polishSegments","sessionId":session,"sources":[
                {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":0,"sourceText":"The demand curve."}
            ],"segments":[{"sourceStart":0,"sourceEnd":1,"text":"The demand curve;"}]}),
        )
        .unwrap();
        let first = &state.sessions[0].segments[0];
        assert_eq!(first.user_seq, 1);
        assert!(first.history.iter().flatten().all(|item| item.automatic));

        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":"r1","startSample":80_000,"endSample":160_000,"text":"Shifts when income changes.","revision":1,"workerEpoch":0,"final":true})).unwrap();
        apply_command(
            &mut state,
            &json!({"type":"polishSegments","sessionId":session,"sources":[
                {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":1,"sourceText":"The demand curve;"},
                {"id":"s2","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"Shifts when income changes."}
            ],"segments":[{"sourceStart":0,"sourceEnd":2,"text":"The demand curve; shifts when income changes."}]}),
        )
        .unwrap();

        assert_eq!(state.sessions[0].segments.len(), 1);
        let survivor = &state.sessions[0].segments[0];
        assert_eq!(survivor.end_sample, 160_000);
        assert_eq!(
            survivor.machine_text,
            "The demand curve; Shifts when income changes."
        );
        assert_eq!(
            survivor.display_text,
            "The demand curve; shifts when income changes."
        );
        assert!(survivor.history.iter().flatten().all(|item| item.automatic));
        let user_seq = survivor.user_seq;
        apply_command(
            &mut state,
            &json!({"type":"undo","sessionId":session,"segmentId":"s1","expectedUserSeq":user_seq}),
        )
        .unwrap();
        assert_eq!(
            state.sessions[0].segments[0].display_text,
            "The demand curve; Shifts when income changes."
        );
        assert_eq!(state.sessions[0].segments[0].end_sample, 160_000);
        assert!(state.sessions[0].segments[0].has_human_history());

        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s3","runId":"r1","startSample":160_000,"endSample":220_000,"text":"A later sentence.","revision":1,"workerEpoch":0,"final":true})).unwrap();
        let before = state.clone();
        assert_eq!(
            apply_command(
                &mut state,
                &json!({"type":"polishSegments","sessionId":session,"sources":[
                    {"id":"s1","expectedMachineRevision":2,"expectedUserSeq":2,"sourceText":"The demand curve; Shifts when income changes."},
                    {"id":"s3","expectedMachineRevision":1,"expectedUserSeq":0,"sourceText":"A later sentence."}
                ],"segments":[{"sourceStart":0,"sourceEnd":2,"text":"The demand curve; shifts when income changes, followed by a later sentence."}]})
            )
            .unwrap_err(),
            "POLISH_MERGE_CONFLICT"
        );
        assert_eq!(state, before);
    }

    #[test]
    fn vocabulary_aliases_apply_to_machine_text_with_safe_word_boundaries() {
        let (mut state, session) = setup();
        state.settings.custom_vocabulary = vec![
            "con yard → Cournot".into(),
            "can man => Kahneman".into(),
            "Nash equilibrium".into(),
        ];
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s1","runId":"r1","startSample":0,"endSample":180000,"text":"Con Yard and can man, but not scan mankind.","revision":2,"workerEpoch":0,"final":false})).unwrap();
        let segment = &state.sessions[0].segments[0];
        assert_eq!(
            segment.machine_text,
            "Cournot and Kahneman, but not scan mankind."
        );
        assert_eq!(segment.display_text, segment.machine_text);
    }

    #[test]
    fn vocabulary_scopes_compose_from_lesson_course_and_global_settings() {
        let (mut state, session) = setup();
        apply_command(
            &mut state,
            &json!({"type":"createProject","title":"Economics"}),
        )
        .unwrap();
        let project = state.projects[0].id.clone();
        apply_command(
            &mut state,
            &json!({"type":"moveSession","sessionId":session,"projectId":project}),
        )
        .unwrap();
        let mut settings = state.settings.clone();
        settings.custom_vocabulary = vec![
            "global error → GlobalTerm".into(),
            "shared error → GlobalShared".into(),
        ];
        apply_command(
            &mut state,
            &json!({
                "type":"settings",
                "settings":settings,
                "projectId":project,
                "projectVocabulary":["course error → CourseTerm","shared error → CourseShared"],
                "sessionId":session,
                "sessionVocabulary":["lesson error → LessonTerm","shared error → LessonShared"]
            }),
        )
        .unwrap();
        let epoch = state.worker_epoch;
        apply_command(&mut state, &json!({"type":"machine","sessionId":session,"segmentId":"s2","runId":"r1","startSample":160000,"endSample":240000,"text":"global error, course error, lesson error, shared error","revision":1,"workerEpoch":epoch,"final":true})).unwrap();
        assert_eq!(
            state.sessions[0].segments[1].display_text,
            "GlobalTerm, CourseTerm, LessonTerm, LessonShared"
        );
        assert_eq!(
            state.projects[0].custom_vocabulary,
            vec!["course error → CourseTerm", "shared error → CourseShared"]
        );
        assert_eq!(
            state.sessions[0].custom_vocabulary,
            vec!["lesson error → LessonTerm", "shared error → LessonShared"]
        );
    }

    #[test]
    fn vocabulary_limits_are_validated() {
        let (mut state, _) = setup();
        let mut settings = state.settings.clone();
        settings.custom_vocabulary = (0..201).map(|index| format!("term {index}")).collect();
        assert_eq!(
            apply_command(&mut state, &json!({"type":"settings","settings":settings})).unwrap_err(),
            "INVALID_VOCABULARY"
        );
    }

    #[test]
    fn appearance_changes_do_not_restart_the_transcription_worker() {
        let (mut state, _) = setup();
        let epoch = state.worker_epoch;
        let mut settings = state.settings.clone();
        settings.theme = "dark".into();
        apply_command(&mut state, &json!({"type":"settings","settings":settings})).unwrap();
        assert_eq!(state.settings.theme, "dark");
        assert_eq!(state.worker_epoch, epoch);
    }
}
