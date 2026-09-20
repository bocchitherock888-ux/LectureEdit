//! In-memory domain store matching `reference/core.py`.
//! Production still needs SQLite, grapheme ranges and a typed event log.

use crate::{merge3, MergeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainError {
    pub code: &'static str,
}

impl DomainError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}

impl std::error::Error for DomainError {}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Correction {
    pub base: String,
    pub base_revision: i64,
    pub text: String,
    pub user_seq: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineHypothesis {
    pub revision: i64,
    pub epoch: i64,
    pub text: String,
    pub final_: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub start_sample: i64,
    pub end_sample: i64,
    pub machine: String,
    pub machine_revision: i64,
    pub final_: bool,
    pub user_seq: i64,
    pub correction: Option<Correction>,
    pub undo_stack: Vec<Option<Correction>>,
    pub machine_history: Vec<MachineHypothesis>,
}

impl Segment {
    pub fn projection(&self) -> MergeResult {
        let Some(c) = &self.correction else {
            return MergeResult {
                text: self.machine.clone(),
                conflict: false,
                pending_machine: None,
                reason: "machine".into(),
            };
        };
        let mut current = MergeResult {
            text: c.text.clone(),
            conflict: false,
            pending_machine: None,
            reason: "human_only".into(),
        };
        for h in self.machine_history.iter().filter(|h| h.revision > c.base_revision) {
            let candidate = merge3(&c.base, &c.text, &h.text);
            current = if candidate.conflict {
                MergeResult {
                    text: current.text,
                    conflict: true,
                    pending_machine: Some(h.text.clone()),
                    reason: candidate.reason.clone(),
                }
            } else {
                candidate
            };
        }
        current
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Editor {
    pub id: String,
    pub segment_id: String,
    pub base: String,
    pub base_revision: i64,
    pub snapshot: String,
    pub expected_user_seq: i64,
    pub draft: String,
    pub draft_revision: i64,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub segment_id: String,
    pub sample: i64,
    pub kind: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitResult {
    pub segment_id: String,
    pub user_seq: i64,
    pub projection: MergeResult,
}

#[derive(Debug, Default)]
pub struct Store {
    pub worker_epoch: i64,
    pub recording: bool,
    pub captured_samples: i64,
    pub segments: HashMap<String, Segment>,
    pub editors: HashMap<String, Editor>,
    pub notes: HashMap<String, Note>,
    command_results: HashMap<String, CommitResult>,
    command_payloads: HashMap<String, (String, String)>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            recording: true,
            ..Self::default()
        }
    }

    pub fn add_segment(&mut self, sid: &str, start: i64, end: i64) -> Result<&Segment, DomainError> {
        if self.segments.contains_key(sid) || start < 0 || end < start {
            return Err(DomainError::new("INVALID_SEGMENT"));
        }
        self.segments.insert(
            sid.to_string(),
            Segment {
                id: sid.to_string(),
                start_sample: start,
                end_sample: end,
                machine: String::new(),
                machine_revision: 0,
                final_: false,
                user_seq: 0,
                correction: None,
                undo_stack: Vec::new(),
                machine_history: Vec::new(),
            },
        );
        Ok(self.segments.get(sid).unwrap())
    }

    pub fn ingest_simulated_frames(&mut self, count: i64) -> Result<(), DomainError> {
        if count < 0 {
            return Err(DomainError::new("INVALID_FRAME_COUNT"));
        }
        if self.recording {
            self.captured_samples += count;
        }
        Ok(())
    }

    pub fn restart_worker(&mut self) -> i64 {
        self.worker_epoch += 1;
        self.worker_epoch
    }

    pub fn hypothesis(
        &mut self,
        sid: &str,
        revision: i64,
        text: &str,
        epoch: i64,
        final_: bool,
        end_sample: Option<i64>,
    ) -> Result<bool, DomainError> {
        let s = self.segments.get_mut(sid).ok_or_else(|| DomainError::new("INVALID_SEGMENT"))?;
        if epoch != self.worker_epoch || revision <= s.machine_revision || s.final_ {
            return Ok(false);
        }
        if let Some(end) = end_sample {
            if end < s.end_sample {
                return Err(DomainError::new("AUDIO_RANGE_SHRANK"));
            }
            s.end_sample = end;
        }
        s.machine = text.to_string();
        s.machine_revision = revision;
        s.final_ = final_;
        s.machine_history.push(MachineHypothesis {
            revision,
            epoch,
            text: text.to_string(),
            final_,
        });
        Ok(true)
    }

    pub fn begin_edit(&mut self, sid: &str) -> Result<Editor, DomainError> {
        let s = self.segments.get(sid).ok_or_else(|| DomainError::new("INVALID_SEGMENT"))?;
        let view = s.projection();
        let (base, base_rev) = if let Some(c) = &s.correction {
            (c.base.clone(), c.base_revision)
        } else {
            (s.machine.clone(), s.machine_revision)
        };
        let e = Editor {
            id: Uuid::new_v4().to_string(),
            segment_id: sid.to_string(),
            base,
            base_revision: base_rev,
            snapshot: view.text.clone(),
            expected_user_seq: s.user_seq,
            draft: view.text,
            draft_revision: 0,
            state: "open".into(),
        };
        self.editors.insert(e.id.clone(), e.clone());
        Ok(e)
    }

    pub fn save_draft(&mut self, editor_id: &str, text: &str, revision: i64) -> Result<bool, DomainError> {
        let e = self.editors.get_mut(editor_id).ok_or_else(|| DomainError::new("EDITOR_CLOSED"))?;
        if e.state != "open" {
            return Err(DomainError::new("EDITOR_CLOSED"));
        }
        if revision < e.draft_revision {
            return Ok(false);
        }
        if revision == e.draft_revision {
            if text != e.draft {
                return Err(DomainError::new("DRAFT_REVISION_REUSED"));
            }
            return Ok(true);
        }
        e.draft = text.to_string();
        e.draft_revision = revision;
        Ok(true)
    }

    pub fn close_editor(&mut self, editor_id: &str) -> Result<(), DomainError> {
        let e = self.editors.get_mut(editor_id).ok_or_else(|| DomainError::new("EDITOR_CLOSED"))?;
        if e.state == "open" {
            e.state = "closed".into();
        }
        Ok(())
    }

    pub fn resume_editor(&mut self, editor_id: &str) -> Result<Editor, DomainError> {
        let e = self.editors.get_mut(editor_id).ok_or_else(|| DomainError::new("EDITOR_NOT_RESUMABLE"))?;
        if e.state != "closed" {
            return Err(DomainError::new("EDITOR_NOT_RESUMABLE"));
        }
        e.state = "open".into();
        Ok(e.clone())
    }

    pub fn discard_draft(&mut self, editor_id: &str) -> Result<(), DomainError> {
        let e = self.editors.get_mut(editor_id).ok_or_else(|| DomainError::new("ALREADY_COMMITTED"))?;
        if e.state == "committed" {
            return Err(DomainError::new("ALREADY_COMMITTED"));
        }
        e.draft.clear();
        e.state = "discarded".into();
        Ok(())
    }

    pub fn commit(&mut self, editor_id: &str, text: &str, command_id: &str) -> Result<CommitResult, DomainError> {
        let payload = (editor_id.to_string(), text.to_string());
        if let Some(existing) = self.command_results.get(command_id) {
            if self.command_payloads.get(command_id) != Some(&payload) {
                return Err(DomainError::new("COMMAND_REUSED_WITH_DIFFERENT_PAYLOAD"));
            }
            return Ok(existing.clone());
        }
        let (sid, expected, snapshot, base, base_rev) = {
            let e = self.editors.get(editor_id).ok_or_else(|| DomainError::new("EDITOR_CLOSED"))?;
            if e.state != "open" {
                return Err(DomainError::new("EDITOR_CLOSED"));
            }
            (e.segment_id.clone(), e.expected_user_seq, e.snapshot.clone(), e.base.clone(), e.base_revision)
        };
        {
            let s = self.segments.get_mut(&sid).unwrap();
            if expected != s.user_seq {
                self.editors.get_mut(editor_id).unwrap().draft = text.to_string();
                return Err(DomainError::new("HUMAN_VERSION_CONFLICT"));
            }
            if text != snapshot {
                s.undo_stack.push(s.correction.clone());
                s.user_seq += 1;
                s.correction = Some(Correction {
                    base,
                    base_revision: base_rev,
                    text: text.to_string(),
                    user_seq: s.user_seq,
                });
            }
        }
        {
            let e = self.editors.get_mut(editor_id).unwrap();
            e.draft = text.to_string();
            e.state = "committed".into();
        }
        let view = self.segments.get(&sid).unwrap().projection();
        let user_seq = self.segments.get(&sid).unwrap().user_seq;
        let result = CommitResult {
            segment_id: sid,
            user_seq,
            projection: view,
        };
        self.command_payloads.insert(command_id.to_string(), payload);
        self.command_results.insert(command_id.to_string(), result.clone());
        Ok(result)
    }

    pub fn undo(&mut self, sid: &str) -> Result<MergeResult, DomainError> {
        let s = self.segments.get_mut(sid).ok_or_else(|| DomainError::new("INVALID_SEGMENT"))?;
        let prev = s.undo_stack.pop().ok_or_else(|| DomainError::new("NOTHING_TO_UNDO"))?;
        s.correction = prev;
        s.user_seq += 1;
        Ok(s.projection())
    }

    pub fn resolve(
        &mut self,
        sid: &str,
        action: &str,
        expected_machine_revision: i64,
        expected_user_seq: i64,
        text: Option<&str>,
    ) -> Result<MergeResult, DomainError> {
        let s = self.segments.get_mut(sid).ok_or_else(|| DomainError::new("INVALID_SEGMENT"))?;
        if s.machine_revision != expected_machine_revision || s.user_seq != expected_user_seq {
            return Err(DomainError::new("RESOLUTION_VERSION_CONFLICT"));
        }
        if !matches!(action, "keep_human" | "accept_machine" | "manual") {
            return Err(DomainError::new("INVALID_RESOLUTION"));
        }
        if action == "manual" && text.is_none() {
            return Err(DomainError::new("MISSING_RESOLUTION_TEXT"));
        }
        let human = s.projection().text;
        s.undo_stack.push(s.correction.clone());
        s.user_seq += 1;
        s.correction = if action == "accept_machine" {
            None
        } else {
            Some(Correction {
                base: s.machine.clone(),
                base_revision: s.machine_revision,
                text: if action == "keep_human" {
                    human
                } else {
                    text.unwrap().to_string()
                },
                user_seq: s.user_seq,
            })
        };
        Ok(s.projection())
    }

    pub fn add_note(&mut self, sid: &str, sample: i64, kind: &str, content: &str) -> Result<Note, DomainError> {
        let s = self.segments.get(sid).ok_or_else(|| DomainError::new("INVALID_NOTE"))?;
        if sample < s.start_sample || sample > s.end_sample || !matches!(kind, "note" | "formula" | "image" | "example") {
            return Err(DomainError::new("INVALID_NOTE"));
        }
        let n = Note {
            id: Uuid::new_v4().to_string(),
            segment_id: sid.to_string(),
            sample,
            kind: kind.into(),
            content: content.into(),
        };
        self.notes.insert(n.id.clone(), n.clone());
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Store {
        let mut store = Store::new();
        store.add_segment("s1", 0, 160000).unwrap();
        store.hypothesis("s1", 1, "According to Talmey, Path is encoded", 0, false, None).unwrap();
        store
    }

    fn edit(store: &mut Store, new_text: Option<&str>, command: &str) -> Editor {
        let e = store.begin_edit("s1").unwrap();
        store
            .commit(&e.id, new_text.unwrap_or("According to Talmy, Path is encoded"), command)
            .unwrap();
        e
    }

    #[test]
    fn draft_does_not_pause_recording() {
        let mut store = setup();
        let e = store.begin_edit("s1").unwrap();
        store.save_draft(&e.id, "my edit", 1).unwrap();
        store.ingest_simulated_frames(32000).unwrap();
        assert!(store.recording);
        assert_eq!(store.captured_samples, 32000);
    }

    #[test]
    fn submit_after_machine_suffix() {
        let mut store = setup();
        let e = store.begin_edit("s1").unwrap();
        let machine = store.segments["s1"].machine.clone();
        store.hypothesis("s1", 2, &format!("{machine} in the verb."), 0, false, None).unwrap();
        store.commit(&e.id, &e.draft.replace("Talmey", "Talmy"), "c1").unwrap();
        assert_eq!(
            store.segments["s1"].projection().text,
            "According to Talmy, Path is encoded in the verb."
        );
    }

    #[test]
    fn later_machine_keeps_user_correction() {
        let mut store = setup();
        edit(&mut store, None, "c1");
        let machine = store.segments["s1"].machine.clone();
        store.hypothesis("s1", 2, &format!("{machine} in the verb."), 0, false, None).unwrap();
        let p = store.segments["s1"].projection();
        assert!(p.text.contains("Talmy"));
        assert!(p.text.ends_with("in the verb."));
    }

    #[test]
    fn old_worker_ignored() {
        let mut store = setup();
        store.restart_worker();
        assert!(!store.hypothesis("s1", 2, "old worker", 0, false, None).unwrap());
        assert!(store.hypothesis("s1", 2, "current worker", 1, false, None).unwrap());
    }

    #[test]
    fn repeated_commit_idempotent() {
        let mut store = setup();
        let e = store.begin_edit("s1").unwrap();
        let a = store.commit(&e.id, "my text", "c1").unwrap();
        let b = store.commit(&e.id, "my text", "c1").unwrap();
        assert_eq!(a.user_seq, b.user_seq);
        assert_eq!(store.segments["s1"].user_seq, 1);
    }

    #[test]
    fn concurrent_human_edit_preserves_draft() {
        let mut store = setup();
        let a = store.begin_edit("s1").unwrap();
        let b = store.begin_edit("s1").unwrap();
        store.commit(&a.id, "first user", "c1").unwrap();
        let err = store.commit(&b.id, "second user", "c2").unwrap_err();
        assert_eq!(err.code, "HUMAN_VERSION_CONFLICT");
        assert_eq!(store.editors[&b.id].draft, "second user");
        assert_eq!(store.segments["s1"].correction.as_ref().unwrap().text, "first user");
    }

    #[test]
    fn undo_retains_machine_continuation() {
        let mut store = setup();
        edit(&mut store, None, "c1");
        let machine = store.segments["s1"].machine.clone();
        store.hypothesis("s1", 2, &format!("{machine} in the verb."), 0, false, None).unwrap();
        let r = store.undo("s1").unwrap();
        assert_eq!(r.text, "According to Talmey, Path is encoded in the verb.");
        assert_eq!(store.segments["s1"].user_seq, 2);
    }

    #[test]
    fn note_anchor_stable() {
        let mut store = setup();
        let n = store.add_note("s1", 80000, "formula", "P(A|B)").unwrap();
        edit(&mut store, None, "c1");
        let machine = store.segments["s1"].machine.clone();
        store.hypothesis("s1", 2, &format!("{machine} more words"), 0, false, None).unwrap();
        assert_eq!(store.notes[&n.id], n);
    }

    #[test]
    fn final_is_terminal() {
        let mut store = setup();
        store.hypothesis("s1", 2, "final", 0, true, None).unwrap();
        assert!(!store.hypothesis("s1", 3, "late partial", 0, false, None).unwrap());
        assert!(!store.hypothesis("s1", 4, "late final", 0, true, None).unwrap());
        assert_eq!(store.segments["s1"].machine, "final");
    }
}
