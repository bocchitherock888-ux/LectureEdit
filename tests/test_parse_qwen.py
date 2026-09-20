import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'spikes' / 'scripts'))
from parse_qwen import parse_qwen_asr


class ParseQwenTests(unittest.TestCase):
    def test_strips_language_tag(self):
        raw = 'language English<asr_text>Yeah. You do through the Moodle.'
        p = parse_qwen_asr(raw)
        self.assertEqual(p['language'], 'English')
        self.assertEqual(p['text'], 'Yeah. You do through the Moodle.')

    def test_empty_body(self):
        p = parse_qwen_asr('language English<asr_text>')
        self.assertEqual(p['language'], 'English')
        self.assertEqual(p['text'], '')

    def test_passthrough_without_tag(self):
        p = parse_qwen_asr('plain text')
        self.assertIsNone(p['language'])
        self.assertEqual(p['text'], 'plain text')


if __name__ == '__main__':
    unittest.main()
