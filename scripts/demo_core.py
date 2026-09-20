"""Simulated transcript/editor sequence. No microphone, network or model is used."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from reference.core import Store

s = Store()
s.add_segment('first', 0, 160000)
s.hypothesis('first', 1, 'According to Talmey, Path is encoded')
e = s.begin_edit('first')
s.save_draft(e.id, e.draft.replace('Talmey', 'Talmy'), 1)
s.ingest_simulated_frames(32000)
s.add_segment('next', 160000, 320000)
s.hypothesis('next', 1, 'The next example concerns English.')
s.hypothesis('first', 2, 'According to Talmey, Path is encoded in the verb.')
result = s.commit(e.id, e.draft, 'save-first')
n = s.add_note('first', 80000, 'formula', r'P(A\mid B)=P(A\cap B)/P(B)')
print('SIMULATION / 固定文字事件，未调用录音或模型')
print('Editing left pane did not alter the simulated recording flag:', s.recording)
print('Simulated additional samples received:', s.captured_samples)
print('Original block:', result['projection']['text'])
print('New block:', s.segments['next'].projection().text)
print('Formula anchor:', n.segment_id, n.sample)
s.hypothesis('first', 3, 'According to Chomsky, Path is encoded in the verb.')
p = s.segments['first'].projection()
print('Later conflict keeps human display:', p.text)
print('Complete pending machine text:', p.pending_machine)
assert 'Talmy' in result['projection']['text']
assert result['projection']['text'].endswith('in the verb.')
assert p.conflict and 'Chomsky' in (p.pending_machine or '')
