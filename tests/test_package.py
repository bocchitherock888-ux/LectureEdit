from pathlib import Path
import json
import re
import unittest
from scripts.smoke_qwen import validate_url

ROOT=Path(__file__).resolve().parents[1]
def luminance(hex_value):
    rgb=[int(hex_value[i:i+2],16)/255 for i in (1,3,5)]
    lin=[x/12.92 if x<=0.04045 else ((x+0.055)/1.055)**2.4 for x in rgb]
    return sum(x*y for x,y in zip(lin,(.2126,.7152,.0722)))
def contrast(a,b):
    x,y=sorted([luminance(a),luminance(b)])
    return (y+.05)/(x+.05)

class PackageTests(unittest.TestCase):
    def test_merge_fixtures_match_reference(self):
        from reference.core import merge3
        cases=json.loads((ROOT/'fixtures/merge_cases.json').read_text(encoding='utf-8'))
        for c in cases:
            result=merge3(c['base'],c['user'],c['machine'])
            self.assertEqual(result.text,c['expected'],c['name'])
            self.assertEqual(result.conflict,c['conflict'],c['name'])
    def test_source_references_exist(self):
        source=(ROOT/'docs/13_来源与核查边界.md').read_text(encoding='utf-8')
        available=set(re.findall(r'## (S\d+) ',source))
        for p in (ROOT/'docs').glob('*.md'):
            for sid in re.findall(r'\[(S\d+)\]',p.read_text(encoding='utf-8')):
                self.assertIn(sid,available,p.name)
    def test_json_files_valid(self):
        # Validate authored project resources; toolchains also emit compressed
        # files with a .json suffix in build/vendor directories.
        import os
        excluded = {'node_modules', 'target', '.native-build', '.git', 'dist',
                    'gen', 'vendor', '.playwright-cli', 'runs'}
        for directory, children, files in os.walk(ROOT):
            children[:] = [name for name in children if name not in excluded]
            for name in files:
                if name.endswith('.json'):
                    p = Path(directory) / name
                    json.loads(p.read_text(encoding='utf-8'))
    def test_unverified_model_cannot_release(self):
        m=json.loads((ROOT/'contracts/model-manifest.template.json').read_text())
        self.assertFalse(m['release_allowed'])
        self.assertEqual(m['verification_status'],'unverified')
        self.assertTrue(all(f['sha256'] is None for f in m['files']))
    def test_core_text_contrast(self):
        for text in ('#232323','#73716D','#3C6E71'):
            for bg in ('#FFFFFF','#F7F7F5'):
                self.assertGreaterEqual(contrast(text,bg),4.5,(text,bg))
    def test_review_text_contrast(self):
        self.assertGreaterEqual(contrast('#765214','#FFF5DF'),4.5)
    def test_local_probe_accepts_only_explicit_loopback(self):
        self.assertEqual(validate_url('http://127.0.0.1:8765/'),'http://127.0.0.1:8765')
        for bad in ('https://example.com','http://0.0.0.0:80','http://localhost:80',
                    'http://127.0.0.1:80/path','http://user@127.0.0.1:80','http://127.0.0.1:80?x=y'):
            with self.assertRaises(ValueError): validate_url(bad)

if __name__ == '__main__': unittest.main()
