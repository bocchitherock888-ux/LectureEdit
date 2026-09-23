use crate::domain::{apply_command, validate_state, State};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS state_snapshot (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    state_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS command_log (
    command_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    committed_at INTEGER NOT NULL DEFAULT (unixepoch())
);
"#;

pub struct Store {
    connection: Connection,
    state: State,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| format!("CREATE_DB_DIR: {error}"))?;
        }
        let connection = Connection::open(path).map_err(db_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
            )
            .map_err(db_error)?;
        connection.execute_batch(SCHEMA).map_err(db_error)?;
        let serialized: Option<String> = connection
            .query_row(
                "SELECT state_json FROM state_snapshot WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        let state = match serialized {
            Some(ref json) => {
                serde_json::from_str(json).map_err(|error| format!("CORRUPT_STATE: {error}"))?
            }
            None => State::default(),
        };
        validate_state(&state)?;
        if serialized.is_none() {
            let json = serde_json::to_string(&state).map_err(json_error)?;
            connection
                .execute(
                    "INSERT INTO state_snapshot(singleton, state_json) VALUES (1, ?1)",
                    [json],
                )
                .map_err(db_error)?;
        }
        Ok(Self { connection, state })
    }

    pub fn snapshot(&self) -> State {
        self.state.clone()
    }

    pub fn dispatch(&mut self, command: Value) -> Result<State, String> {
        let kind = command
            .get("type")
            .and_then(Value::as_str)
            .ok_or("INVALID_type")?;
        if kind == "snapshot" {
            return Ok(self.snapshot());
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
            return Ok(self.snapshot());
        }

        let mut next = self.state.clone();
        if let Err(error) = apply_command(&mut next, &command) {
            // The attempted text remains recoverable when another human edit won
            // the compare-and-swap race. The rejected command itself is retryable.
            if error == "HUMAN_VERSION_CONFLICT" && next != self.state {
                validate_state(&next)?;
                self.persist_state(&next)?;
                self.state = next;
            }
            return Err(error);
        }
        validate_state(&next)?;
        let state_json = serde_json::to_string(&next).map_err(json_error)?;
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
        transaction
            .execute(
                "UPDATE state_snapshot SET state_json = ?1 WHERE singleton = 1",
                [&state_json],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        self.state = next;
        Ok(self.snapshot())
    }

    pub fn mutate<F>(&mut self, function: F) -> Result<State, String>
    where
        F: FnOnce(&mut State) -> Result<(), String>,
    {
        let mut next = self.state.clone();
        function(&mut next)?;
        validate_state(&next)?;
        self.persist_state(&next)?;
        self.state = next;
        Ok(self.snapshot())
    }

    fn persist_state(&mut self, state: &State) -> Result<(), String> {
        let json = serde_json::to_string(state).map_err(json_error)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE state_snapshot SET state_json = ?1 WHERE singleton = 1",
                [&json],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)
    }
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

    #[test]
    fn legacy_snapshot_defaults_projects_and_session_membership() {
        let db = path();
        let mut store = Store::open(&db).unwrap();
        let created = store
            .dispatch(
                json!({"type":"createSession","commandId":"create","title":"Legacy","mode":"live"}),
            )
            .unwrap();
        let mut legacy = serde_json::to_value(created).unwrap();
        legacy.as_object_mut().unwrap().remove("projects");
        for session in legacy["sessions"].as_array_mut().unwrap() {
            session.as_object_mut().unwrap().remove("projectId");
        }
        store
            .connection
            .execute(
                "UPDATE state_snapshot SET state_json = ?1 WHERE singleton = 1",
                [serde_json::to_string(&legacy).unwrap()],
            )
            .unwrap();
        drop(store);

        let restored = Store::open(&db).unwrap().snapshot();
        assert!(restored.projects.is_empty());
        assert_eq!(restored.sessions[0].project_id, None);
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
