-- Initial schema, version 1. Domain rules are additionally enforced in the writer actor.
PRAGMA foreign_keys=ON;
PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
PRAGMA busy_timeout=5000;
CREATE TABLE schema_migrations (
 version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL, description TEXT NOT NULL
) STRICT;
CREATE TABLE sessions (
 id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at_ms INTEGER NOT NULL,
 language TEXT NOT NULL DEFAULT 'auto',
 state TEXT NOT NULL CHECK(state IN ('idle','recording','stopped','recovering','archived')),
 operation_seq INTEGER NOT NULL DEFAULT 0 CHECK(operation_seq>=0)
) STRICT;
CREATE TABLE capture_runs (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 source_kind TEXT NOT NULL CHECK(source_kind IN ('microphone','system')),
 device_label TEXT NOT NULL, started_at_ms INTEGER NOT NULL, ended_at_ms INTEGER,
 session_offset_ms INTEGER NOT NULL CHECK(session_offset_ms>=0),
 sample_rate INTEGER NOT NULL DEFAULT 16000 CHECK(sample_rate=16000),
 channels INTEGER NOT NULL DEFAULT 1 CHECK(channels=1),
 state TEXT NOT NULL CHECK(state IN ('starting','recording','closed','interrupted')),
 CHECK(ended_at_ms IS NULL OR ended_at_ms>=started_at_ms)
) STRICT;
CREATE TABLE audio_chunks (
 id TEXT PRIMARY KEY, capture_run_id TEXT NOT NULL REFERENCES capture_runs(id) ON DELETE CASCADE,
 start_sample INTEGER NOT NULL CHECK(start_sample>=0),
 end_sample INTEGER NOT NULL CHECK(end_sample>=start_sample),
 relative_path TEXT NOT NULL UNIQUE,
 byte_length INTEGER NOT NULL CHECK(byte_length>=0), sha256 TEXT,
 state TEXT NOT NULL CHECK(state IN ('writing','closed','recovered','missing')),
 CHECK(byte_length=(end_sample-start_sample)*2),
 CHECK(relative_path NOT LIKE '/%' AND instr(relative_path,'..')=0 AND instr(relative_path,char(92))=0),
 UNIQUE(capture_run_id,start_sample)
) STRICT;
CREATE TABLE gaps (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 capture_run_id TEXT REFERENCES capture_runs(id) ON DELETE CASCADE,
 session_start_ms INTEGER NOT NULL CHECK(session_start_ms>=0),
 session_end_ms INTEGER NOT NULL CHECK(session_end_ms>=session_start_ms),
 reason TEXT NOT NULL, details_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(details_json))
) STRICT;
CREATE TABLE segments (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 capture_run_id TEXT NOT NULL REFERENCES capture_runs(id) ON DELETE CASCADE,
 start_sample INTEGER NOT NULL CHECK(start_sample>=0),
 end_sample INTEGER NOT NULL CHECK(end_sample>=start_sample),
 phase TEXT NOT NULL CHECK(phase IN ('provisional','final')),
 latest_machine_revision INTEGER NOT NULL DEFAULT 0 CHECK(latest_machine_revision>=0),
 user_seq INTEGER NOT NULL DEFAULT 0 CHECK(user_seq>=0),
 active_user_revision_id TEXT REFERENCES user_revisions(id) DEFERRABLE INITIALLY DEFERRED,
 tombstone INTEGER NOT NULL DEFAULT 0 CHECK(tombstone IN (0,1))
) STRICT;
CREATE TABLE machine_hypotheses (
 segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL CHECK(revision>0),
 worker_epoch INTEGER NOT NULL CHECK(worker_epoch>=0), text TEXT NOT NULL,
 is_final INTEGER NOT NULL CHECK(is_final IN (0,1)), created_at_ms INTEGER NOT NULL,
 PRIMARY KEY(segment_id,revision)
) STRICT;
CREATE TABLE user_revisions (
 id TEXT PRIMARY KEY, segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
 parent_id TEXT REFERENCES user_revisions(id), user_seq INTEGER NOT NULL CHECK(user_seq>0),
 base_machine_revision INTEGER NOT NULL CHECK(base_machine_revision>=0),
 base_machine_text TEXT NOT NULL, user_text TEXT NOT NULL,
 action TEXT NOT NULL CHECK(action IN ('edit','keep_human','manual_resolution','undo','accept_machine')),
 created_at_ms INTEGER NOT NULL, UNIQUE(segment_id,user_seq)
) STRICT;
CREATE TABLE drafts (
 editor_id TEXT PRIMARY KEY, segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
 base_machine_revision INTEGER NOT NULL CHECK(base_machine_revision>=0), base_machine_text TEXT NOT NULL,
 snapshot_display_text TEXT NOT NULL, expected_user_seq INTEGER NOT NULL CHECK(expected_user_seq>=0),
 draft_revision INTEGER NOT NULL CHECK(draft_revision>=0), text TEXT NOT NULL,
 updated_at_ms INTEGER NOT NULL, state TEXT NOT NULL CHECK(state IN ('open','closed','committed','discarded'))
) STRICT;
CREATE TABLE assets (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 sha256 TEXT NOT NULL, relative_path TEXT NOT NULL UNIQUE,
 mime TEXT NOT NULL CHECK(mime IN ('image/png','image/jpeg','image/webp')),
 byte_length INTEGER NOT NULL CHECK(byte_length>=0),
 width INTEGER NOT NULL CHECK(width>0), height INTEGER NOT NULL CHECK(height>0),
 created_at_ms INTEGER NOT NULL,
 CHECK(relative_path NOT LIKE '/%' AND instr(relative_path,'..')=0 AND instr(relative_path,char(92))=0),
 UNIQUE(session_id,sha256)
) STRICT;
CREATE TABLE note_blocks (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 anchor_segment_id TEXT NOT NULL REFERENCES segments(id),
 capture_run_id TEXT NOT NULL REFERENCES capture_runs(id), anchor_sample INTEGER NOT NULL CHECK(anchor_sample>=0),
 side TEXT NOT NULL CHECK(side IN ('before','after')), rank TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('note','formula','image','example')),
 content_json TEXT NOT NULL CHECK(json_valid(content_json)),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
 tombstone INTEGER NOT NULL DEFAULT 0 CHECK(tombstone IN (0,1)),
 created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE note_asset_links (
 note_id TEXT NOT NULL REFERENCES note_blocks(id) ON DELETE CASCADE,
 asset_id TEXT NOT NULL REFERENCES assets(id), PRIMARY KEY(note_id,asset_id)
) STRICT;
CREATE TABLE operations (
 command_id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 operation_seq INTEGER NOT NULL CHECK(operation_seq>0), kind TEXT NOT NULL,
 result_json TEXT NOT NULL CHECK(json_valid(result_json)), committed_at_ms INTEGER NOT NULL,
 UNIQUE(session_id,operation_seq)
) STRICT;
CREATE TABLE jobs (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
 worker_epoch INTEGER NOT NULL CHECK(worker_epoch>=0),
 kind TEXT NOT NULL CHECK(kind IN ('partial','final','reprocess')), backend_id TEXT NOT NULL,
 range_json TEXT NOT NULL CHECK(json_valid(range_json)),
 state TEXT NOT NULL CHECK(state IN ('queued','running','done','cancelled','failed')),
 attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0), error_code TEXT, updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL CHECK(json_valid(value_json))) STRICT;
CREATE TABLE glossary_entries (
 id TEXT PRIMARY KEY, session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
 spelling TEXT NOT NULL, language TEXT NOT NULL, created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX segments_by_audio ON segments(session_id,capture_run_id,start_sample);
CREATE INDEX notes_by_anchor ON note_blocks(anchor_segment_id,side,rank);
CREATE INDEX jobs_pending ON jobs(state,kind,updated_at_ms);
CREATE INDEX machine_recent ON machine_hypotheses(segment_id,revision DESC);

-- Explicit user intent outlives accidental later machine agreement.
CREATE TABLE protected_edits (
 id TEXT PRIMARY KEY, user_revision_id TEXT NOT NULL REFERENCES user_revisions(id) ON DELETE CASCADE,
 segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
 base_machine_revision INTEGER NOT NULL CHECK(base_machine_revision>=0),
 start_grapheme INTEGER NOT NULL CHECK(start_grapheme>=0),
 end_grapheme INTEGER NOT NULL CHECK(end_grapheme>=start_grapheme),
 replacement TEXT NOT NULL, lineage_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(lineage_json))
) STRICT;
