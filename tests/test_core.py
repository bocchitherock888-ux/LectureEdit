import random
import unittest
from reference.core import DomainError, Store, merge3


class MergeTests(unittest.TestCase):
    def test_no_change(self):
        self.assertEqual(merge3('hello', 'hello', 'hello').text, 'hello')
    def test_human_only(self):
        self.assertEqual(merge3('Talmey', 'Talmy', 'Talmey').text, 'Talmy')
    def test_machine_only(self):
        self.assertEqual(merge3('Path', 'Path', 'Path continues').text, 'Path continues')
    def test_same_change_once(self):
        self.assertEqual(merge3('old', 'new', 'new').text, 'new')
    def test_growing_suffix(self):
        r = merge3('According to Talmey, Path is encoded', 'According to Talmy, Path is encoded',
                   'According to Talmey, Path is encoded in the verb.')
        self.assertFalse(r.conflict)
        self.assertEqual(r.text, 'According to Talmy, Path is encoded in the verb.')
    def test_conflicting_number(self):
        r = merge3('It is fifteen percent.', 'It is fifty percent.', 'It is sixteen percent.')
        self.assertTrue(r.conflict)
        self.assertEqual(r.text, 'It is fifty percent.')
        self.assertEqual(r.pending_machine, 'It is sixteen percent.')
    def test_conflict_retains_full_suffix(self):
        r = merge3('It is fifteen percent.', 'It is fifty percent.', 'It is sixteen percent. More follows.')
        self.assertEqual(r.pending_machine, 'It is sixteen percent. More follows.')
    def test_deduplicates_shared_patch(self):
        r = merge3('old word', 'new word', 'new word continues')
        self.assertFalse(r.conflict)
        self.assertEqual(r.text, 'new word continues')
    def test_disjoint_words(self):
        self.assertEqual(merge3('alpha beta gamma', 'ALPHA beta gamma', 'alpha beta GAMMA').text,
                         'ALPHA beta GAMMA')
    def test_same_insertion_position_conflicts(self):
        self.assertTrue(merge3('a b', 'a x b', 'a y b').conflict)
    def test_repeated_word_ambiguous(self):
        r = merge3('path links path', 'Path links path', 'path links path today')
        self.assertTrue(r.conflict)
        self.assertEqual(r.reason, 'ambiguous_alignment')
    def test_chinese_preserved(self):
        r = merge3('他研究路径', '她研究路径', '他研究路径编码')
        self.assertFalse(r.conflict)
        self.assertEqual(r.text, '她研究路径编码')
    def test_newlines_and_spaces(self):
        r = merge3('alpha  beta\nend', 'ALPHA  beta\nend', 'alpha  beta\nEND')
        self.assertEqual(r.text, 'ALPHA  beta\nEND')
    def test_empty_base(self):
        r = merge3('', 'user', 'machine')
        self.assertTrue(r.conflict)
    def test_delete_all_and_new_machine(self):
        self.assertTrue(merge3('a b', '', 'a b c').conflict)
    def test_type_validation(self):
        with self.assertRaises(TypeError):
            merge3(None, '', '')
    def test_combining_character_retained(self):
        human = 'Cafe\u0301 term'
        r = merge3('Cafe term', human, 'Cafe term today')
        self.assertIn('e\u0301', r.text)
    def test_random_disjoint_replacements(self):
        rng = random.Random(20260918)
        for _ in range(500):
            words = [f'token{i}' for i in range(18)]
            i, j = rng.sample(range(18), 2)
            u, m, expected = words.copy(), words.copy(), words.copy()
            u[i] = f'HUMAN{i}'; m[j] = f'MACHINE{j}'
            expected[i], expected[j] = u[i], m[j]
            r = merge3(' '.join(words), ' '.join(u), ' '.join(m))
            self.assertFalse(r.conflict)
            self.assertEqual(r.text, ' '.join(expected))


class StateTests(unittest.TestCase):
    def setUp(self):
        self.store = Store()
        self.s = self.store.add_segment('s1', 0, 160000)
        self.store.hypothesis('s1', 1, 'According to Talmey, Path is encoded')
    def edit(self, new_text=None, command='c1'):
        e = self.store.begin_edit('s1')
        self.store.commit(e.id, new_text or 'According to Talmy, Path is encoded', command)
        return e
    def test_draft_does_not_pause_recording(self):
        e = self.store.begin_edit('s1')
        self.store.save_draft(e.id, 'my edit', 1)
        self.store.ingest_simulated_frames(32000)
        self.assertTrue(self.store.recording)
        self.assertEqual(self.store.captured_samples, 32000)
    def test_other_segments_continue(self):
        self.store.begin_edit('s1')
        self.store.add_segment('s2', 160000, 320000)
        self.store.hypothesis('s2', 1, 'Next point.')
        self.assertEqual(self.store.segments['s2'].projection().text, 'Next point.')
    def test_editor_snapshot_frozen(self):
        e = self.store.begin_edit('s1')
        self.store.hypothesis('s1', 2, self.s.machine + ' in the verb.')
        self.assertEqual(e.draft, 'According to Talmey, Path is encoded')
    def test_submit_after_machine_suffix(self):
        e = self.store.begin_edit('s1')
        self.store.hypothesis('s1', 2, self.s.machine + ' in the verb.')
        self.store.commit(e.id, e.draft.replace('Talmey', 'Talmy'), 'c1')
        self.assertEqual(self.s.projection().text, 'According to Talmy, Path is encoded in the verb.')
    def test_later_machine_keeps_user_correction(self):
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine + ' in the verb.')
        self.assertIn('Talmy', self.s.projection().text)
        self.assertTrue(self.s.projection().text.endswith('in the verb.'))
    def test_duplicate_hypothesis(self):
        self.assertFalse(self.store.hypothesis('s1', 1, 'wrong'))
    def test_out_of_order_hypothesis(self):
        self.store.hypothesis('s1', 3, 'new')
        self.assertFalse(self.store.hypothesis('s1', 2, 'old'))
        self.assertEqual(self.s.machine, 'new')
    def test_final_is_terminal(self):
        self.store.hypothesis('s1', 2, 'final', final=True)
        self.assertFalse(self.store.hypothesis('s1', 3, 'late partial'))
        self.assertFalse(self.store.hypothesis('s1', 4, 'late final', final=True))
        self.assertEqual(self.s.machine, 'final')
    def test_old_worker_ignored(self):
        self.store.restart_worker()
        self.assertFalse(self.store.hypothesis('s1', 2, 'old worker', epoch=0))
        self.assertTrue(self.store.hypothesis('s1', 2, 'current worker', epoch=1))
    def test_repeated_commit_idempotent(self):
        e = self.store.begin_edit('s1')
        a = self.store.commit(e.id, 'my text', 'c1')
        b = self.store.commit(e.id, 'my text', 'c1')
        self.assertEqual(a, b)
        self.assertEqual(self.s.user_seq, 1)
    def test_command_payload_reuse_rejected(self):
        e = self.edit()
        with self.assertRaises(DomainError) as ctx:
            self.store.commit(e.id, 'different', 'c1')
        self.assertEqual(ctx.exception.code, 'COMMAND_REUSED_WITH_DIFFERENT_PAYLOAD')
    def test_concurrent_human_edit_preserves_draft(self):
        a, b = self.store.begin_edit('s1'), self.store.begin_edit('s1')
        self.store.commit(a.id, 'first user', 'c1')
        with self.assertRaises(DomainError):
            self.store.commit(b.id, 'second user', 'c2')
        self.assertEqual(b.draft, 'second user')
        self.assertEqual(self.s.correction.text, 'first user')
    def test_no_change_does_not_create_correction(self):
        e = self.store.begin_edit('s1')
        self.store.hypothesis('s1', 2, 'machine changed')
        self.store.commit(e.id, e.snapshot, 'c1')
        self.assertEqual(self.s.projection().text, 'machine changed')
        self.assertIsNone(self.s.correction)
        self.assertEqual(self.s.user_seq, 0)
    def test_close_keeps_draft(self):
        e = self.store.begin_edit('s1')
        self.store.save_draft(e.id, 'saved draft', 1)
        self.store.close_editor(e.id)
        self.assertEqual(self.store.resume_editor(e.id).draft, 'saved draft')
    def test_discard_is_explicit(self):
        e = self.store.begin_edit('s1')
        self.store.discard_draft(e.id)
        self.assertEqual(e.state, 'discarded')
        self.assertEqual(e.draft, '')
    def test_stale_draft_ignored(self):
        e = self.store.begin_edit('s1')
        self.store.save_draft(e.id, 'new', 2)
        self.assertFalse(self.store.save_draft(e.id, 'old', 1))
        self.assertEqual(e.draft, 'new')
    def test_draft_revision_collision(self):
        e = self.store.begin_edit('s1')
        self.store.save_draft(e.id, 'new', 1)
        with self.assertRaises(DomainError):
            self.store.save_draft(e.id, 'different', 1)
    def test_undo_retains_machine_continuation(self):
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine + ' in the verb.')
        r = self.store.undo('s1')
        self.assertEqual(r.text, 'According to Talmey, Path is encoded in the verb.')
        self.assertEqual(self.s.user_seq, 2)
    def test_note_anchor_stable(self):
        n = self.store.add_note('s1', 80000, 'formula', 'P(A|B)')
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine + ' more words')
        self.assertEqual(self.store.notes[n.id], n)
    def test_note_outside_audio_rejected(self):
        with self.assertRaises(DomainError):
            self.store.add_note('s1', 900000, 'note', 'bad')
    def test_reedit_preserves_pending_lineage(self):
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine.replace('Talmey', 'Chomsky'))
        self.assertTrue(self.s.projection().conflict)
        e = self.store.begin_edit('s1')
        self.store.commit(e.id, e.draft.replace('Path', 'PATH'), 'c2')
        self.assertTrue(self.s.projection().conflict)
        self.assertIn('Talmy', self.s.projection().text)
        self.assertIn('Chomsky', self.s.projection().pending_machine)
    def test_keep_human_resolves_current_candidate(self):
        self.edit()
        self.store.hypothesis('s1', 2, 'a very different transcript')
        r = self.store.resolve('s1', 'keep_human', 2, 1)
        self.assertFalse(r.conflict)
        self.assertIn('Talmy', r.text)
    def test_accept_machine_explicit(self):
        self.edit()
        self.store.hypothesis('s1', 2, 'replacement')
        r = self.store.resolve('s1', 'accept_machine', 2, 1)
        self.assertEqual(r.text, 'replacement')
        self.assertIsNone(self.s.correction)
    def test_resolution_version_check(self):
        self.edit()
        with self.assertRaises(DomainError):
            self.store.resolve('s1', 'keep_human', 99, 1)
    def test_snapshot_roundtrip_preserves_draft_and_idempotency(self):
        done = self.edit()
        e = self.store.begin_edit('s1')
        self.store.save_draft(e.id, 'unfinished', 1)
        restored = Store.loads(self.store.dumps())
        self.assertEqual(restored.editors[e.id].draft, 'unfinished')
        self.assertEqual(restored.segments['s1'].projection(), self.s.projection())
        self.assertEqual(restored.commit(done.id, done.draft, 'c1')['user_seq'], 1)
        self.assertFalse(restored.recording)
    def test_audio_range_cannot_shrink(self):
        with self.assertRaises(DomainError):
            self.store.hypothesis('s1', 2, 'text', end_sample=159999)

    def test_safe_continuation_remains_visible_on_later_conflict(self):
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine + ' in the verb.')
        self.store.hypothesis('s1', 3, self.s.machine.replace('Talmey', 'Chomsky'))
        r = self.s.projection()
        self.assertTrue(r.conflict)
        self.assertEqual(r.text, 'According to Talmy, Path is encoded in the verb.')
    def test_machine_agreement_does_not_erase_human_protection(self):
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine.replace('Talmey', 'Talmy'))
        self.store.hypothesis('s1', 3, self.s.machine.replace('Talmy', 'Chomsky'))
        self.assertTrue(self.s.projection().conflict)
        self.assertIn('Talmy', self.s.projection().text)
    def test_reedit_after_agreement_retains_earlier_human_intent(self):
        self.edit()
        self.store.hypothesis('s1', 2, self.s.machine.replace('Talmey', 'Talmy'))
        e = self.store.begin_edit('s1')
        self.store.commit(e.id, e.draft.replace('Path', 'PATH'), 'c2')
        self.store.hypothesis('s1', 3, self.s.machine.replace('Talmy', 'Chomsky'))
        self.assertTrue(self.s.projection().conflict)
        self.assertIn('Talmy', self.s.projection().text)
        self.assertIn('PATH', self.s.projection().text)

    def test_growing_range_keeps_id_and_note(self):
        n = self.store.add_note('s1', 100, 'note', 'example')
        self.store.hypothesis('s1', 2, 'continued', end_sample=192000)
        self.assertEqual(self.s.id, 's1')
        self.assertEqual(self.store.notes[n.id].segment_id, 's1')
    def test_long_simulated_event_sequence(self):
        # 7,200 seconds of timestamps, not a real-time or model performance test.
        for i in range(720):
            sid = f'long-{i}'
            self.store.add_segment(sid, i*160000, (i+1)*160000)
            self.store.hypothesis(sid, 1, f'block {i}')
            self.store.hypothesis(sid, 2, f'block {i} final', final=True)
        self.assertEqual(len(self.store.segments), 721)
        self.assertTrue(all(s.final for k,s in self.store.segments.items() if k.startswith('long-')))

if __name__ == '__main__':
    unittest.main()
