use crate::domain::{
    apply_command, validate_session, validate_state, Project, Session, Settings, State,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

// Each session lives in its own row so a live transcript update rewrites one
// lecture, not the whole library. `state_snapshot` is the pre-0.1.4 layout and
// is migrated once, then emptied.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS state_snapshot (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    state_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS library_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    meta_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS session_rows (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    session_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS quarantine (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    reason TEXT NOT NULL,
    payload TEXT NOT NULL,
    quarantined_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE IF NOT EXISTS command_log (
    command_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    committed_at INTEGER NOT NULL DEFAULT (unixepoch())
);
"#;

/// Receipts only need to outlive client retries, which happen within seconds.
const COMMAND_LOG_RETENTION_SECS: i64 = 14 * 24 * 60 * 60;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetaRef<'a> {
    schema_version: u32,
    projects: &'a [Project],
    selected_session_id: &'a Option<String>,
    settings: &'a Settings,
    worker_epoch: u64,
}

fn meta_json(state: &State) -> Result<String, String> {
    serde_json::to_string(&MetaRef {
        schema_version: state.schema_version,
        projects: &state.projects,
        selected_session_id: &state.selected_session_id,
        settings: &state.settings,
        worker_epoch: state.worker_epoch,
    })
    .map_err(json_error)
}

fn meta_changed(before: &State, after: &State) -> bool {
    before.schema_version != after.schema_version
        || before.projects != after.projects
        || before.selected_session_id != after.selected_session_id
        || before.settings != after.settings
        || before.worker_epoch != after.worker_epoch
}

pub struct Store {
    connection: Connection,
    state: State,
    recovered: Vec<String>,
    versions: Versions,
}

/// Change counters for incremental sync: the UI asks for what changed since
/// the revision it holds instead of re-reading the whole library.
struct Versions {
    epoch: String,
    revision: u64,
    meta: u64,
    sessions: HashMap<String, u64>,
}

struct Changes {
    meta: bool,
    sessions: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Delta<'a> {
    epoch: &'a str,
    revision: u64,
    full: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<MetaRef<'a>>,
    order: Vec<&'a str>,
    sessions: Vec<&'a Session>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| format!("CREATE_DB_DIR: {error}"))?;
        }
        let mut connection = Connection::open(path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
            )
            .map_err(db_error)?;
        connection.execute_batch(SCHEMA).map_err(db_error)?;
        let _ = connection.execute(
            "DELETE FROM command_log WHERE committed_at < unixepoch() - ?1",
            [COMMAND_LOG_RETENTION_SECS],
        );

        let mut recovered = Vec::new();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let meta: Option<String> = transaction
            .query_row(
                "SELECT meta_json FROM library_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let state = match meta {
            Some(meta) => load_rows(&transaction, &meta, &mut recovered)?,
            None => {
                let legacy: Option<String> = transaction
                    .query_row(
                        "SELECT state_json FROM state_snapshot WHERE singleton = 1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db_error)?;
                let state = match legacy {
                    Some(json) => recover_legacy(&transaction, &json, &mut recovered)?,
                    None => State::default(),
                };
                write_all(&transaction, &state)?;
                transaction
                    .execute("DELETE FROM state_snapshot", [])
                    .map_err(db_error)?;
                state
            }
        };
        validate_state(&state)?;
        transaction.commit().map_err(db_error)?;
        let versions = Versions {
            epoch: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            meta: 1,
            sessions: state.sessions.iter().map(|s| (s.id.clone(), 1)).collect(),
        };
        Ok(Self {
            connection,
            state,
            recovered,
            versions,
        })
    }

    fn record(&mut self, changes: Changes) {
        if !changes.meta && changes.sessions.is_empty() {
            return;
        }
        self.versions.revision += 1;
        let revision = self.versions.revision;
        if changes.meta {
            self.versions.meta = revision;
        }
        for id in changes.sessions {
            self.versions.sessions.insert(id, revision);
        }
        let live: HashSet<&str> = self.state.sessions.iter().map(|s| s.id.as_str()).collect();
        self.versions.sessions.retain(|id, _| live.contains(id.as_str()));
    }

    /// Everything that changed after `since` in this process (`epoch`), or the
    /// whole library when the caller holds nothing usable.
    pub fn delta_json(&self, epoch: &str, since: u64) -> Result<Box<serde_json::value::RawValue>, String> {
        let versions = &self.versions;
        let full = epoch != versions.epoch || since == 0 || since > versions.revision;
        let delta = Delta {
            epoch: &versions.epoch,
            revision: versions.revision,
            full,
            meta: (full || versions.meta > since).then(|| MetaRef {
                schema_version: self.state.schema_version,
                projects: &self.state.projects,
                selected_session_id: &self.state.selected_session_id,
                settings: &self.state.settings,
                worker_epoch: self.state.worker_epoch,
            }),
            order: self.state.sessions.iter().map(|s| s.id.as_str()).collect(),
            sessions: self
                .state
                .sessions
                .iter()
                .filter(|s| full || versions.sessions.get(&s.id).is_none_or(|rev| *rev > since))
                .collect(),
        };
        serde_json::value::to_raw_value(&delta).map_err(json_error)
    }

    /// Opens the library; if SQLite reports the file itself as damaged, the
    /// file is renamed aside (never deleted) and a fresh library is started.
    pub fn open_or_recover(path: &Path) -> Result<Self, String> {
        match Self::open(path) {
            Ok(store) => Ok(store),
            Err(error) if is_corrupt_database(&error) => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or(0);
                for suffix in ["", "-wal", "-shm"] {
                    let from = path.with_file_name(format!(
                        "{}{suffix}",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("lectureedit.sqlite")
                    ));
                    if from.exists() {
                        let to = from.with_file_name(format!(
                            "{}.damaged-{stamp}",
                            from.file_name().and_then(|n| n.to_str()).unwrap_or("lectureedit.sqlite")
                        ));
                        std::fs::rename(&from, &to)
                            .map_err(|rename| format!("{error}; 无法移开损坏的资料库：{rename}"))?;
                    }
                }
                let mut store = Self::open(path)?;
                store.recovered.push("整个资料库".into());
                Ok(store)
            }
            Err(error) => Err(error),
        }
    }

    pub fn snapshot(&self) -> State {
        self.state.clone()
    }

    /// Borrow the current state without cloning the whole library.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Sessions that could not be read at startup; they are kept in the
    /// `quarantine` table instead of blocking the app.
    pub fn recovered(&self) -> &[String] {
        &self.recovered
    }

    pub fn dispatch(&mut self, command: Value) -> Result<State, String> {
        self.apply(command)?;
        Ok(self.snapshot())
    }

    /// Like `dispatch`, without cloning the library for a caller that does
    /// not read it back (the transcription hot path).
    pub fn apply(&mut self, command: Value) -> Result<(), String> {
        let kind = command
            .get("type")
            .and_then(Value::as_str)
            .ok_or("INVALID_type")?;
        if kind == "snapshot" {
            return Ok(());
        }
        let command_id = command
            .get("commandId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 200)
            .ok_or("INVALID_commandId")?
            .to_string();
        let payload = payload_hash(&command)?;
        if let Some((saved_payload, _receipt)) = self
            .connection
            .query_row(
                "SELECT payload_json, result_json FROM command_log WHERE command_id = ?1",
                [&command_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(db_error)?
        {
            if saved_payload != payload {
                return Err("COMMAND_REUSED_WITH_DIFFERENT_PAYLOAD".into());
            }
            return Ok(());
        }

        let mut next = self.state.clone();
        if let Err(error) = apply_command(&mut next, &command) {
            // The attempted text remains recoverable when another human edit won
            // the compare-and-swap race. The rejected command itself is retryable.
            if error == "HUMAN_VERSION_CONFLICT" && next != self.state {
                validate_state(&next)?;
                let changes = self.persist_state(&next)?;
                self.state = next;
                self.record(changes);
            }
            return Err(error);
        }
        validate_state(&next)?;
        let session_id = command
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let operation_seq = next
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.operation_seq);
        let receipt = serde_json::to_string(&serde_json::json!({
            "type": kind,
            "sessionId": if session_id.is_empty() { None } else { Some(session_id) },
            "operationSeq": operation_seq
        }))
        .map_err(json_error)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute(
                "INSERT INTO command_log(command_id, payload_json, result_json) VALUES (?1, ?2, ?3)",
                params![command_id, payload, receipt],
            )
            .map_err(db_error)?;
        let changes = write_changes(&transaction, &self.state, &next)?;
        transaction.commit().map_err(db_error)?;
        self.state = next;
        self.record(changes);
        Ok(())
    }

    pub fn mutate<F>(&mut self, function: F) -> Result<State, String>
    where
        F: FnOnce(&mut State) -> Result<(), String>,
    {
        self.change(function)?;
        Ok(self.snapshot())
    }

    /// Like `mutate`, without cloning the library for the return value.
    pub fn change<F>(&mut self, function: F) -> Result<(), String>
    where
        F: FnOnce(&mut State) -> Result<(), String>,
    {
        let mut next = self.state.clone();
        function(&mut next)?;
        validate_state(&next)?;
        let changes = self.persist_state(&next)?;
        self.state = next;
        self.record(changes);
        Ok(())
    }

    fn persist_state(&mut self, state: &State) -> Result<Changes, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let changes = write_changes(&transaction, &self.state, state)?;
        transaction.commit().map_err(db_error)?;
        Ok(changes)
    }
}

fn write_all(transaction: &Transaction, state: &State) -> Result<(), String> {
    transaction
        .execute(
            "INSERT OR REPLACE INTO library_meta(singleton, meta_json) VALUES (1, ?1)",
            [meta_json(state)?],
        )
        .map_err(db_error)?;
    transaction
        .execute("DELETE FROM session_rows", [])
        .map_err(db_error)?;
    for (position, session) in state.sessions.iter().enumerate() {
        upsert_session(transaction, position, session)?;
    }
    Ok(())
}

/// Writes only what differs between `before` (what is on disk) and `after`.
fn write_changes(transaction: &Transaction, before: &State, after: &State) -> Result<Changes, String> {
    let mut changes = Changes {
        meta: meta_changed(before, after),
        sessions: Vec::new(),
    };
    if changes.meta {
        transaction
            .execute(
                "UPDATE library_meta SET meta_json = ?1 WHERE singleton = 1",
                [meta_json(after)?],
            )
            .map_err(db_error)?;
    }
    let previous: HashMap<&str, (usize, &Session)> = before
        .sessions
        .iter()
        .enumerate()
        .map(|(position, session)| (session.id.as_str(), (position, session)))
        .collect();
    for (position, session) in after.sessions.iter().enumerate() {
        match previous.get(session.id.as_str()) {
            Some((_, old)) if *old != session => {
                upsert_session(transaction, position, session)?;
                changes.sessions.push(session.id.clone());
            }
            Some((old_position, _)) if *old_position != position => {
                transaction
                    .execute(
                        "UPDATE session_rows SET position = ?1 WHERE id = ?2",
                        params![position as i64, session.id],
                    )
                    .map_err(db_error)?;
            }
            Some(_) => {}
            None => {
                upsert_session(transaction, position, session)?;
                changes.sessions.push(session.id.clone());
            }
        }
    }
    let kept: HashSet<&str> = after.sessions.iter().map(|s| s.id.as_str()).collect();
    for id in previous.keys().filter(|id| !kept.contains(*id)) {
        transaction
            .execute("DELETE FROM session_rows WHERE id = ?1", [id])
            .map_err(db_error)?;
        changes.meta = true;
    }
    Ok(changes)
}

fn upsert_session(transaction: &Transaction, position: usize, session: &Session) -> Result<(), String> {
    let json = serde_json::to_string(session).map_err(json_error)?;
    transaction
        .execute(
            "INSERT INTO session_rows(id, position, session_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET position = excluded.position, session_json = excluded.session_json",
            params![session.id, position as i64, json],
        )
        .map_err(db_error)?;
    Ok(())
}

fn quarantine(transaction: &Transaction, source: &str, reason: &str, payload: &str) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO quarantine(source, reason, payload) VALUES (?1, ?2, ?3)",
            params![source, reason, payload],
        )
        .map_err(db_error)?;
    Ok(())
}

/// Builds the library meta from JSON, falling back to defaults for a part that
/// cannot be read rather than refusing to start.
fn parse_meta(mut meta: Value, recovered: &mut Vec<String>) -> State {
    let object = meta.as_object_mut().map(std::mem::take).unwrap_or_default();
    let mut state = State::default();
    if let Some(Value::Number(n)) = object.get("schemaVersion") {
        state.schema_version = n.as_u64().unwrap_or(1) as u32;
    }
    if let Some(value) = object.get("projects") {
        match serde_json::from_value::<Vec<Project>>(value.clone()) {
            Ok(projects) => {
                let mut seen = HashSet::new();
                state.projects = projects
                    .into_iter()
                    .filter(|project| seen.insert(project.id.clone()))
                    .collect();
            }
            Err(_) => recovered.push("课程文件夹".into()),
        }
    }
    if let Some(value) = object.get("settings") {
        match serde_json::from_value::<Settings>(value.clone()) {
            Ok(settings) => state.settings = settings,
            Err(_) => recovered.push("设置".into()),
        }
    }
    state.selected_session_id = object
        .get("selectedSessionId")
        .and_then(Value::as_str)
        .map(str::to_string);
    state.worker_epoch = object
        .get("workerEpoch")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    state
}

/// Adds a session if it is readable and consistent; otherwise sets it aside.
fn admit_session(
    transaction: &Transaction,
    state: &mut State,
    seen: &mut HashSet<String>,
    source: &str,
    raw: &str,
    recovered: &mut Vec<String>,
) -> Result<bool, String> {
    let label = || {
        serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| value.get("title").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "一节课".into())
    };
    let mut session = match serde_json::from_str::<Session>(raw) {
        Ok(session) => session,
        Err(error) => {
            quarantine(transaction, source, &format!("JSON: {error}"), raw)?;
            recovered.push(label());
            return Ok(false);
        }
    };
    if let Err(error) = validate_session(&session) {
        quarantine(transaction, source, &error, raw)?;
        recovered.push(label());
        return Ok(false);
    }
    if !seen.insert(session.id.clone()) {
        quarantine(transaction, source, "DUPLICATE_SESSION", raw)?;
        return Ok(false);
    }
    let mut repaired = false;
    if session
        .project_id
        .as_deref()
        .is_some_and(|id| !state.projects.iter().any(|project| project.id == id))
    {
        session.project_id = None;
        repaired = true;
    }
    state.sessions.push(session);
    Ok(repaired)
}

fn finish_recovery(state: &mut State) {
    if state
        .selected_session_id
        .as_ref()
        .is_some_and(|id| !state.sessions.iter().any(|session| &session.id == id))
    {
        state.selected_session_id = state.sessions.first().map(|session| session.id.clone());
    }
}

fn load_rows(transaction: &Transaction, meta: &str, recovered: &mut Vec<String>) -> Result<State, String> {
    let meta = serde_json::from_str::<Value>(meta).unwrap_or_else(|_| {
        recovered.push("资料库设置".into());
        Value::Null
    });
    let mut state = parse_meta(meta, recovered);
    let rows: Vec<(String, String)> = {
        let mut statement = transaction
            .prepare("SELECT id, session_json FROM session_rows ORDER BY position, id")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_error)?
            .collect::<Result<_, _>>()
            .map_err(db_error)?;
        rows
    };
    let mut seen = HashSet::new();
    let mut changed = false;
    for (id, raw) in rows {
        let before = state.sessions.len();
        changed |= admit_session(transaction, &mut state, &mut seen, "session_rows", &raw, recovered)?;
        if state.sessions.len() == before {
            changed = true;
            transaction
                .execute("DELETE FROM session_rows WHERE id = ?1", [&id])
                .map_err(db_error)?;
        }
    }
    finish_recovery(&mut state);
    if validate_state(&state).is_err() {
        // Only the meta can still be wrong here; keep the lectures.
        recovered.push("资料库设置".into());
        let sessions = std::mem::take(&mut state.sessions);
        state = State {
            sessions,
            ..State::default()
        };
        finish_recovery(&mut state);
    }
    if changed || !recovered.is_empty() {
        write_all(transaction, &state)?;
    } else {
        transaction
            .execute(
                "UPDATE library_meta SET meta_json = ?1 WHERE singleton = 1",
                [meta_json(&state)?],
            )
            .map_err(db_error)?;
    }
    Ok(state)
}

fn recover_legacy(transaction: &Transaction, json: &str, recovered: &mut Vec<String>) -> Result<State, String> {
    if let Ok(state) = serde_json::from_str::<State>(json) {
        if validate_state(&state).is_ok() {
            return Ok(state);
        }
    }
    let Ok(mut value) = serde_json::from_str::<Value>(json) else {
        quarantine(transaction, "state_snapshot", "UNREADABLE_JSON", json)?;
        recovered.push("整个资料库".into());
        return Ok(State::default());
    };
    let sessions = value
        .get_mut("sessions")
        .map(Value::take)
        .and_then(|value| match value {
            Value::Array(items) => Some(items),
            _ => None,
        })
        .unwrap_or_default();
    let mut state = parse_meta(value, recovered);
    let mut seen = HashSet::new();
    for item in sessions {
        let raw = serde_json::to_string(&item).map_err(json_error)?;
        admit_session(transaction, &mut state, &mut seen, "state_snapshot", &raw, recovered)?;
    }
    finish_recovery(&mut state);
    if validate_state(&state).is_err() {
        recovered.push("资料库设置".into());
        let sessions = std::mem::take(&mut state.sessions);
        state = State {
            sessions,
            ..State::default()
        };
        finish_recovery(&mut state);
    }
    Ok(state)
}

fn is_corrupt_database(error: &str) -> bool {
    error.starts_with("SQLITE:")
        && (error.contains("malformed") || error.contains("not a database"))
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLITE: {error}")
}

fn json_error(error: serde_json::Error) -> String {
    format!("JSON: {error}")
}

fn payload_hash(command: &Value) -> Result<String, String> {
    let canonical = serde_json::to_vec(&canonical_value(command)).map_err(json_error)?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        Value::Object(items) => {
            let mut keys: Vec<&String> = items.keys().collect();
            keys.sort_unstable();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical_value(&items[key]));
            }
            Value::Object(sorted)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use uuid::Uuid;

    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lectureedit-store-{}.sqlite3", Uuid::new_v4()))
    }

    fn legacy_db(json: &str) -> std::path::PathBuf {
        let db = path();
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE state_snapshot (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), state_json TEXT NOT NULL);
                 CREATE TABLE command_log (command_id TEXT PRIMARY KEY, payload_json TEXT NOT NULL, result_json TEXT NOT NULL, committed_at INTEGER NOT NULL DEFAULT (unixepoch()));",
            )
            .unwrap();
        connection
            .execute("INSERT INTO state_snapshot(singleton, state_json) VALUES (1, ?1)", [json])
            .unwrap();
        db
    }

    fn sample_state() -> State {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        store.dispatch(json!({"type":"createSession","commandId":"a","title":"Markets","mode":"live"})).unwrap();
        let state = store.dispatch(json!({"type":"createSession","commandId":"b","title":"Choices","mode":"live"})).unwrap();
        let sid = state.selected_session_id.clone().unwrap();
        let state = store.dispatch(json!({"type":"machine","commandId":"m","sessionId":sid,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":16000,"text":"Supply and demand","revision":1,"workerEpoch":0,"final":true})).unwrap();
        drop(store);
        let _ = fs::remove_file(db);
        state
    }

    #[test]
    fn legacy_snapshot_defaults_projects_and_session_membership() {
        let mut legacy = serde_json::to_value(sample_state()).unwrap();
        legacy.as_object_mut().unwrap().remove("projects");
        for session in legacy["sessions"].as_array_mut().unwrap() {
            session.as_object_mut().unwrap().remove("projectId");
        }
        let db = legacy_db(&serde_json::to_string(&legacy).unwrap());
        let restored = Store::open(&db).unwrap().snapshot();
        assert!(restored.projects.is_empty());
        assert_eq!(restored.sessions.len(), 2);
        assert_eq!(restored.sessions[0].project_id, None);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn legacy_snapshot_migrates_once_into_session_rows() {
        let original = sample_state();
        let db = legacy_db(&serde_json::to_string(&original).unwrap());
        {
            let store = Store::open(&db).unwrap();
            assert_eq!(store.snapshot(), original);
            assert!(store.recovered().is_empty());
            let legacy: i64 = store.connection.query_row("SELECT COUNT(*) FROM state_snapshot", [], |r| r.get(0)).unwrap();
            let rows: i64 = store.connection.query_row("SELECT COUNT(*) FROM session_rows", [], |r| r.get(0)).unwrap();
            assert_eq!((legacy, rows), (0, 2));
        }
        let mut reopened = Store::open(&db).unwrap();
        assert_eq!(reopened.snapshot(), original);
        let sid = original.sessions[1].id.clone();
        reopened.dispatch(json!({"type":"renameSession","commandId":"r","sessionId":sid,"title":"Renamed"})).ok();
        drop(reopened);
        let again = Store::open(&db).unwrap().snapshot();
        assert_eq!(again.sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), original.sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>());
        let _ = fs::remove_file(db);
    }

    #[test]
    fn an_edit_rewrites_only_the_changed_session() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        store.dispatch(json!({"type":"createSession","commandId":"a","title":"Markets","mode":"live"})).unwrap();
        let state = store.dispatch(json!({"type":"createSession","commandId":"b","title":"Choices","mode":"live"})).unwrap();
        let sid = state.selected_session_id.unwrap();
        let other = state.sessions.iter().find(|s| s.id != sid).unwrap().id.clone();
        store.connection.execute("UPDATE session_rows SET session_json = session_json || ' ' WHERE id = ?1", [&other]).unwrap();
        let before = store.connection.total_changes();
        store.dispatch(json!({"type":"machine","commandId":"m","sessionId":sid,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":16000,"text":"hello","revision":1,"workerEpoch":0,"final":false})).unwrap();
        assert_eq!(store.connection.total_changes() - before, 2, "command receipt plus one session row");
        let untouched: String = store.connection.query_row("SELECT session_json FROM session_rows WHERE id = ?1", [&other], |r| r.get(0)).unwrap();
        assert!(untouched.ends_with(' '), "the other lecture was not rewritten");
        drop(store);
        let restored = Store::open(&db).unwrap().snapshot();
        assert_eq!(restored.sessions.len(), 2);
        assert_eq!(restored.sessions.iter().find(|s| s.id == sid).unwrap().segments.len(), 1);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn a_corrupt_lecture_is_quarantined_and_the_rest_open() {
        let db = path();
        let (keep, broken);
        {
            let mut store = Store::open(&db).unwrap();
            let a = store.dispatch(json!({"type":"createSession","commandId":"a","title":"Markets","mode":"live"})).unwrap();
            keep = a.selected_session_id.unwrap();
            let b = store.dispatch(json!({"type":"createSession","commandId":"b","title":"Choices","mode":"live"})).unwrap();
            broken = b.selected_session_id.unwrap();
            store.connection.execute("UPDATE session_rows SET session_json = '{\"title\":\"Choices\",\"segments\":' WHERE id = ?1", [&broken]).unwrap();
        }
        let store = Store::open(&db).unwrap();
        assert_eq!(store.state().sessions.len(), 1);
        assert_eq!(store.state().sessions[0].id, keep);
        assert_eq!(store.state().selected_session_id.as_deref(), Some(keep.as_str()));
        assert_eq!(store.recovered(), ["一节课"]);
        let held: i64 = store.connection.query_row("SELECT COUNT(*) FROM quarantine", [], |r| r.get(0)).unwrap();
        assert_eq!(held, 1);
        drop(store);
        let again = Store::open(&db).unwrap();
        assert!(again.recovered().is_empty(), "recovery is reported once");
        let _ = fs::remove_file(db);
    }

    fn delta(store: &Store, epoch: &str, since: u64) -> Value {
        serde_json::from_str(store.delta_json(epoch, since).unwrap().get()).unwrap()
    }

    #[test]
    fn sync_sends_only_what_changed_since_the_callers_revision() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        store.dispatch(json!({"type":"createSession","commandId":"a","title":"Markets","mode":"live"})).unwrap();
        let state = store.dispatch(json!({"type":"createSession","commandId":"b","title":"Choices","mode":"live"})).unwrap();
        let sid = state.selected_session_id.unwrap();

        let first = delta(&store, "", 0);
        assert_eq!(first["full"], true);
        assert_eq!(first["sessions"].as_array().unwrap().len(), 2);
        assert!(first["meta"]["settings"].is_object());
        let epoch = first["epoch"].as_str().unwrap().to_string();
        let held = first["revision"].as_u64().unwrap();

        let idle = delta(&store, &epoch, held);
        assert_eq!(idle["full"], false);
        assert!(idle["sessions"].as_array().unwrap().is_empty());
        assert!(idle.get("meta").is_none());
        assert_eq!(idle["order"].as_array().unwrap().len(), 2);

        store.apply(json!({"type":"machine","commandId":"m","sessionId":sid,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":16000,"text":"hello","revision":1,"workerEpoch":0,"final":false})).unwrap();
        let changed = delta(&store, &epoch, held);
        assert_eq!(changed["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(changed["sessions"][0]["id"], sid.as_str());
        assert!(changed.get("meta").is_none());
        assert!(changed["revision"].as_u64().unwrap() > held);

        // A failed command changes nothing and bumps nothing.
        let revision = changed["revision"].as_u64().unwrap();
        assert!(store.apply(json!({"type":"moveSession","commandId":"bad","sessionId":sid,"projectId":"missing"})).is_err());
        assert_eq!(delta(&store, &epoch, revision)["revision"].as_u64().unwrap(), revision);

        // Another process (or a restart) never trusts an old revision.
        assert_eq!(delta(&store, "stale-epoch", revision)["full"], true);
        drop(store);
        let reopened = Store::open(&db).unwrap();
        assert_eq!(delta(&reopened, &epoch, revision)["full"], true);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn a_damaged_database_file_is_set_aside_not_deleted() {
        let db = path();
        fs::write(&db, b"this is definitely not an sqlite database, just some bytes that fill the header area").unwrap();
        assert!(Store::open(&db).is_err());
        let store = Store::open_or_recover(&db).unwrap();
        assert_eq!(store.recovered(), ["整个资料库"]);
        let kept: Vec<_> = fs::read_dir(db.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&format!("{}.damaged-", db.file_name().unwrap().to_string_lossy())))
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(fs::read(kept[0].path()).unwrap().starts_with(b"this is definitely"));
        let _ = fs::remove_file(kept[0].path());
        drop(store);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn an_unreadable_legacy_library_opens_empty_and_is_kept() {
        let db = legacy_db("{not json");
        let store = Store::open(&db).unwrap();
        assert!(store.state().sessions.is_empty());
        assert_eq!(store.recovered(), ["整个资料库"]);
        let payload: String = store.connection.query_row("SELECT payload FROM quarantine", [], |r| r.get(0)).unwrap();
        assert_eq!(payload, "{not json");
        let _ = fs::remove_file(db);
    }

    #[test]
    fn a_session_pointing_at_a_missing_project_is_repaired() {
        let mut legacy = serde_json::to_value(sample_state()).unwrap();
        legacy["sessions"][0]["projectId"] = json!("gone");
        let db = legacy_db(&serde_json::to_string(&legacy).unwrap());
        let store = Store::open(&db).unwrap();
        assert_eq!(store.state().sessions.len(), 2);
        assert_eq!(store.state().sessions[0].project_id, None);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn project_membership_survives_restart_and_invalid_moves_roll_back() {
        let db = path();
        let (project_id, session_id, before_invalid);
        {
            let mut store = Store::open(&db).unwrap();
            let created_project = store
                .dispatch(json!({"type":"createProject","commandId":"project","title":"Economics"}))
                .unwrap();
            project_id = created_project.projects[0].id.clone();
            store
                .dispatch(json!({"type":"renameProject","commandId":"rename-project","projectId":project_id,"title":"Applied Economics"}))
                .unwrap();
            let created_session = store
                .dispatch(json!({"type":"createSession","commandId":"session","title":"Markets","mode":"live"}))
                .unwrap();
            session_id = created_session.selected_session_id.unwrap();
            let moved = store
                .dispatch(json!({"type":"moveSession","commandId":"move","sessionId":session_id,"projectId":project_id}))
                .unwrap();
            assert_eq!(moved.sessions[0].id, session_id);
            assert_eq!(
                moved.sessions[0].project_id.as_deref(),
                Some(project_id.as_str())
            );
            before_invalid = moved;
            assert_eq!(
                store
                    .dispatch(json!({"type":"moveSession","commandId":"bad-move","sessionId":session_id,"projectId":"missing"}))
                    .unwrap_err(),
                "PROJECT_NOT_FOUND"
            );
            assert_eq!(store.snapshot(), before_invalid);
        }
        let restored = Store::open(&db).unwrap().snapshot();
        assert_eq!(restored.projects[0].id, project_id);
        assert_eq!(restored.projects[0].title, "Applied Economics");
        assert_eq!(restored.sessions[0].id, session_id);
        assert_eq!(
            restored.sessions[0].project_id.as_deref(),
            Some(project_id.as_str())
        );
        let _ = fs::remove_file(db);
    }

    fn command(id: &str, kind: &str) -> Value {
        json!({"type": kind, "commandId": id})
    }

    #[test]
    fn restart_preserves_state_and_draft() {
        let db = path();
        let draft_id;
        {
            let mut store = Store::open(&db).unwrap();
            let created = store.dispatch(json!({"type":"createSession","commandId":"create","title":"Syntax","mode":"live"})).unwrap();
            let session_id = created.selected_session_id.unwrap();
            store.dispatch(json!({"type":"machine","commandId":"m1","sessionId":session_id,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":16000,"text":"Talmey speaks","revision":1,"workerEpoch":0,"final":false})).unwrap();
            let begun = store.dispatch(json!({"type":"beginEdit","commandId":"begin","sessionId":session_id,"segmentId":"seg-1"})).unwrap();
            draft_id = begun.sessions[0].drafts[0].id.clone();
            store.dispatch(json!({"type":"saveDraft","commandId":"save","sessionId":session_id,"draftId":draft_id,"text":"Talmy speaks","revision":1})).unwrap();
        }
        let restored = Store::open(&db).unwrap().snapshot();
        assert_eq!(restored.sessions[0].drafts[0].text, "Talmy speaks");
        let _ = fs::remove_file(db);
    }

    #[test]
    fn failed_mutation_rolls_back_memory_and_disk() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        let before = store.snapshot();
        assert_eq!(
            store.mutate(|state| {
                state.worker_epoch = 99;
                Err("FAULT".into())
            }),
            Err("FAULT".into())
        );
        assert_eq!(store.snapshot(), before);
        drop(store);
        assert_eq!(Store::open(&db).unwrap().snapshot(), before);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn command_retry_is_idempotent_and_payload_mismatch_is_rejected() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        let value = json!({"type":"createSession","commandId":"same","title":"A","mode":"live"});
        let first = store.dispatch(value.clone()).unwrap();
        drop(store);
        let mut store = Store::open(&db).unwrap();
        assert_eq!(store.dispatch(value).unwrap(), first);
        assert_eq!(store.snapshot().sessions.len(), 1);
        let error = store
            .dispatch(json!({"type":"createSession","commandId":"same","title":"B","mode":"live"}))
            .unwrap_err();
        assert_eq!(error, "COMMAND_REUSED_WITH_DIFFERENT_PAYLOAD");
        assert_eq!(store.snapshot().sessions.len(), 1);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn human_cas_conflict_preserves_attempted_draft_across_restart() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        let created = store
            .dispatch(
                json!({"type":"createSession","commandId":"create","title":"Syntax","mode":"live"}),
            )
            .unwrap();
        let session_id = created.selected_session_id.unwrap();
        store.dispatch(json!({"type":"machine","commandId":"m1","sessionId":session_id,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":16000,"text":"original","revision":1,"workerEpoch":0,"final":false})).unwrap();
        let first = store.dispatch(json!({"type":"beginEdit","commandId":"b1","sessionId":session_id,"segmentId":"seg-1"})).unwrap();
        let first_id = first.sessions[0].drafts[0].id.clone();
        let second = store.dispatch(json!({"type":"beginEdit","commandId":"b2","sessionId":session_id,"segmentId":"seg-1"})).unwrap();
        let second_id = second.sessions[0].drafts[1].id.clone();
        store.dispatch(json!({"type":"commitEdit","commandId":"c1","sessionId":session_id,"draftId":first_id,"text":"first human","expectedUserSeq":0})).unwrap();
        let error = store.dispatch(json!({"type":"commitEdit","commandId":"c2","sessionId":session_id,"draftId":second_id,"text":"second human","expectedUserSeq":0})).unwrap_err();
        assert_eq!(error, "HUMAN_VERSION_CONFLICT");
        assert_eq!(
            store.snapshot().sessions[0].segments[0].display_text,
            "first human"
        );
        drop(store);
        let restored = Store::open(&db).unwrap().snapshot();
        assert_eq!(restored.sessions[0].drafts[1].text, "second human");
        let _ = fs::remove_file(db);
    }

    #[test]
    fn idempotent_commit_retry_returns_current_machine_projection() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        let created = store
            .dispatch(
                json!({"type":"createSession","commandId":"create","title":"Syntax","mode":"live"}),
            )
            .unwrap();
        let session_id = created.selected_session_id.unwrap();
        store.dispatch(json!({"type":"machine","commandId":"m1","sessionId":session_id,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":16000,"text":"Talmey speaks","revision":1,"workerEpoch":0,"final":false})).unwrap();
        let begun = store.dispatch(json!({"type":"beginEdit","commandId":"begin","sessionId":session_id,"segmentId":"seg-1"})).unwrap();
        let draft_id = begun.sessions[0].drafts[0].id.clone();
        let commit = json!({"type":"commitEdit","commandId":"commit","sessionId":session_id,"draftId":draft_id,"text":"Talmy speaks","expectedUserSeq":0});
        store.dispatch(commit.clone()).unwrap();
        store.dispatch(json!({"type":"machine","commandId":"m2","sessionId":session_id,"segmentId":"seg-1","runId":"run-1","startSample":0,"endSample":32000,"text":"Talmey speaks today","revision":2,"workerEpoch":0,"final":false})).unwrap();
        let retry = store.dispatch(commit).unwrap();
        assert_eq!(
            retry.sessions[0].segments[0].display_text,
            "Talmy speaks today"
        );
        assert_eq!(retry.sessions[0].segments[0].user_seq, 1);
        let _ = fs::remove_file(db);
    }

    #[test]
    fn unknown_command_does_not_change_state() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        let before = store.snapshot();
        assert_eq!(
            store.dispatch(command("bad", "unknown")).unwrap_err(),
            "UNKNOWN_COMMAND"
        );
        assert_eq!(store.snapshot(), before);
        let _ = fs::remove_file(db);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use serde_json::json;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn bench_machine_dispatch_on_large_library() {
        let db = std::env::temp_dir().join(format!("lectureedit-bench-{}.sqlite3", uuid::Uuid::new_v4()));
        let mut store = Store::open(&db).unwrap();
        let sentence = "Opportunity cost is the value of the best alternative we give up when making a choice. ";
        let created = store.dispatch(json!({"type":"createSession","commandId":"c0","title":"Lecture","mode":"live"})).unwrap();
        let first = created.selected_session_id.unwrap();
        for i in 0..1500u64 {
            store.dispatch(json!({"type":"machine","commandId":format!("seed{i}"),"sessionId":first,"segmentId":format!("r_{i}"),"runId":"r","startSample":i*48000,"endSample":i*48000+40000,"text":sentence,"revision":1,"workerEpoch":0,"final":true})).unwrap();
        }
        store.mutate(|state| {
            let template = state.sessions[0].clone();
            for s in 1..40 {
                let mut copy = template.clone();
                copy.id = format!("copy-{s}");
                state.sessions.push(copy);
            }
            Ok(())
        }).unwrap();
        let sid = store.state().sessions[0].id.clone();
        let size = serde_json::to_string(store.state()).unwrap().len();
        let t = Instant::now();
        let n = 30;
        for i in 0..n {
            store.apply(json!({"type":"machine","commandId":format!("m{i}"),"sessionId":sid,"segmentId":format!("live_{i}"),"runId":"live","startSample":100_000_000u64 + i*16000,"endSample":100_000_000u64 + i*16000+8000,"text":"hello there","revision":1,"workerEpoch":0,"final":false})).unwrap_or_else(|e| panic!("{e}"));
        }
        eprintln!("library {:.1} MB, machine dispatch avg {:.1} ms", size as f64 / 1e6, t.elapsed().as_secs_f64() * 1000.0 / n as f64);
        let t = Instant::now();
        for i in 0..5u64 {
            store.mutate(|s| { let mut v = serde_json::to_value(&*s).unwrap(); v["sessions"][0]["title"] = json!(format!("a{i}")); *s = serde_json::from_value(v).unwrap(); Ok(()) }).unwrap();
        }
        eprintln!("old per-second run sync {:.1} ms", t.elapsed().as_secs_f64()*200.0);
        let t = Instant::now();
        for i in 0..5u64 {
            store.mutate(|s| { let slot = &mut s.sessions[0]; let mut v = serde_json::to_value(&*slot).unwrap(); v["title"] = json!(format!("b{i}")); *slot = serde_json::from_value(v).unwrap(); Ok(()) }).unwrap();
        }
        eprintln!("new per-second run sync {:.1} ms", t.elapsed().as_secs_f64()*200.0);
        let t = Instant::now(); for _ in 0..5 { std::hint::black_box(store.state().clone()); } eprintln!("clone {:.1} ms", t.elapsed().as_secs_f64()*200.0);
        let t = Instant::now(); for _ in 0..5 { crate::domain::validate_state(store.state()).unwrap(); } eprintln!("validate {:.1} ms", t.elapsed().as_secs_f64()*200.0);
        let t = Instant::now(); for _ in 0..5 { std::hint::black_box(serde_json::to_string(store.state()).unwrap()); } eprintln!("serialize {:.1} ms", t.elapsed().as_secs_f64()*200.0);
        let t = Instant::now(); for _ in 0..5 { std::hint::black_box(store.state() == &store.state().clone()); } eprintln!("clone+eq {:.1} ms", t.elapsed().as_secs_f64()*200.0);
        let one = serde_json::to_string(&store.state().sessions[0]).unwrap(); eprintln!("one session {:.2} MB", one.len() as f64/1e6);
        let _ = std::fs::remove_file(db);
    }
}
