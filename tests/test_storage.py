import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class SchemaTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.db = sqlite3.connect(str(Path(self.temp.name)/'test.sqlite3'))
        self.db.executescript((ROOT/'contracts/schema.sql').read_text(encoding='utf-8'))
        self.db.execute("INSERT INTO sessions VALUES ('course','Class',1,'en','recording',0)")
        self.db.execute("INSERT INTO capture_runs VALUES ('run','course','microphone','test',1,NULL,0,16000,1,'recording')")
        self.db.execute("INSERT INTO segments (id,session_id,capture_run_id,start_sample,end_sample,phase) VALUES ('seg','course','run',0,160000,'provisional')")
        self.db.commit()
    def tearDown(self):
        self.db.close(); self.temp.cleanup()
    def test_wal_and_full(self):
        self.assertEqual(self.db.execute('PRAGMA journal_mode').fetchone()[0], 'wal')
        self.assertEqual(self.db.execute('PRAGMA synchronous').fetchone()[0], 2)
    def test_integrity(self):
        self.assertEqual(self.db.execute('PRAGMA integrity_check').fetchone()[0], 'ok')
        self.assertEqual(self.db.execute('PRAGMA foreign_key_check').fetchall(), [])
    def test_orphan_machine_rejected(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO machine_hypotheses VALUES ('missing',1,0,'text',0,1)")
    def test_range_invalid(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("UPDATE segments SET end_sample=-1 WHERE id='seg'")
    def test_audio_size_check(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO audio_chunks VALUES ('c','run',0,160,'audio/c.pcm',999,NULL,'closed')")
    def test_path_traversal_check(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO audio_chunks VALUES ('c','run',0,160,'../c.pcm',320,NULL,'closed')")
    def test_json_invalid(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO settings VALUES ('key','{')")
    def test_duplicate_command_rejected(self):
        self.db.execute("INSERT INTO operations VALUES ('cmd','course',1,'edit','{}',1)")
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO operations VALUES ('cmd','course',2,'edit','{}',2)")
    def test_transaction_rollback(self):
        self.db.execute('BEGIN')
        self.db.execute("INSERT INTO user_revisions VALUES ('u','seg',NULL,1,1,'bad','good','edit',1)")
        self.db.execute("UPDATE segments SET active_user_revision_id='u',user_seq=1 WHERE id='seg'")
        self.db.rollback()
        self.assertEqual(self.db.execute('SELECT count(*) FROM user_revisions').fetchone()[0], 0)
        self.assertEqual(self.db.execute('SELECT user_seq FROM segments').fetchone()[0], 0)
    def test_user_revision_and_pointer_transaction(self):
        with self.db:
            self.db.execute("INSERT INTO user_revisions VALUES ('u','seg',NULL,1,1,'bad','good','edit',1)")
            self.db.execute("UPDATE segments SET active_user_revision_id='u',user_seq=1 WHERE id='seg'")
            self.db.execute("INSERT INTO operations VALUES ('cmd','course',1,'edit','{\"user_seq\":1}',1)")
        self.assertEqual(self.db.execute('PRAGMA foreign_key_check').fetchall(), [])
    def test_backup_contains_committed_data(self):
        self.db.execute("INSERT INTO machine_hypotheses VALUES ('seg',1,0,'text',1,1)")
        self.db.commit()
        target = sqlite3.connect(str(Path(self.temp.name)/'backup.sqlite3'))
        try:
            self.db.backup(target)
            self.assertEqual(target.execute('SELECT text FROM machine_hypotheses').fetchone()[0], 'text')
        finally:
            target.close()
    def test_course_delete_cascades(self):
        with self.db:
            self.db.execute("INSERT INTO note_blocks VALUES ('n','course','seg','run',100,'after','a','formula','{}',1,0,1,1)")
            self.db.execute("DELETE FROM sessions WHERE id='course'")
        self.assertEqual(self.db.execute('SELECT count(*) FROM segments').fetchone()[0], 0)
        self.assertEqual(self.db.execute('SELECT count(*) FROM note_blocks').fetchone()[0], 0)
        self.assertEqual(self.db.execute('PRAGMA foreign_key_check').fetchall(), [])

if __name__ == '__main__': unittest.main()
