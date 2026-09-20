"""Reference merge and editor state semantics, using only the Python standard library.

Production still requires durable transactions, native recording, grapheme-aware editor
mapping, complete redo/event protocols and measured model integration. Ambiguous diffs
are deliberately returned for human review. This is structural, not semantic, merging.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import asdict, dataclass, field
from difflib import SequenceMatcher
import json
import re
from typing import Any
from uuid import uuid4

_TOKEN = re.compile(r"[\u3400-\u9fff]|\w+|\s+|[^\w\s]", re.UNICODE)


@dataclass(frozen=True)
class Patch:
    start: int
    end: int
    replacement: tuple[str, ...]


@dataclass(frozen=True)
class MergeResult:
    text: str
    conflict: bool = False
    pending_machine: str | None = None
    reason: str = 'clean'


def _patches(base: list[str], changed: list[str]) -> list[Patch]:
    return [Patch(i, j, tuple(changed[x:y]))
            for tag, i, j, x, y in SequenceMatcher(None, base, changed, autojunk=False).get_opcodes()
            if tag != 'equal']


def _overlap(a: Patch, b: Patch) -> bool:
    # Boundaries are conservative: simultaneous edits at an insertion edge need review.
    if a.start == a.end:
        return b.start <= a.start <= b.end
    if b.start == b.end:
        return a.start <= b.start <= a.end
    return max(a.start, b.start) < min(a.end, b.end)


def _count_sequence(base: list[str], needle: list[str]) -> int:
    if not needle:
        return 0
    return sum(base[i:i + len(needle)] == needle for i in range(len(base) - len(needle) + 1))


def _ambiguous(base: list[str], p: Patch) -> bool:
    if p.start != p.end:
        return _count_sequence(base, base[p.start:p.end]) > 1
    # Beginning/end insertion is anchored to the whole segment boundary.
    if p.start in (0, len(base)):
        return False
    context = base[max(0, p.start - 4):min(len(base), p.start + 4)]
    return _count_sequence(base, context) > 1


def merge3(base: str, user: str, machine: str) -> MergeResult:
    """Apply disjoint token patches; retain complete alternatives on any uncertainty."""
    if not all(isinstance(s, str) for s in (base, user, machine)):
        raise TypeError('merge3 expects strings')
    if user == machine:
        return MergeResult(user, reason='identical_branches')
    if user == base:
        return MergeResult(machine, reason='machine_only')
    if machine == base:
        return MergeResult(user, reason='human_only')
    b = _TOKEN.findall(base)
    up = _patches(b, _TOKEN.findall(user))
    mp = _patches(b, _TOKEN.findall(machine))
    for a in up:
        for c in mp:
            if a != c and _overlap(a, c):
                return MergeResult(user, True, machine, 'overlapping_edits')
    common = set(up).intersection(mp)
    if any(_ambiguous(b, p) for p in [*up, *mp] if p not in common):
        return MergeResult(user, True, machine, 'ambiguous_alignment')
    merged = list(b)
    for p in sorted(set([*up, *mp]), key=lambda x: (x.start, x.end), reverse=True):
        merged[p.start:p.end] = p.replacement
    return MergeResult(''.join(merged))


class DomainError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


@dataclass(frozen=True)
class Correction:
    base: str
    base_revision: int
    text: str
    user_seq: int


@dataclass
class Segment:
    id: str
    start_sample: int
    end_sample: int
    machine: str = ''
    machine_revision: int = 0
    final: bool = False
    user_seq: int = 0
    correction: Correction | None = None
    undo_stack: list[Correction | None] = field(default_factory=list)
    machine_history: list[dict[str, Any]] = field(default_factory=list)

    def projection(self) -> MergeResult:
        c = self.correction
        if c is None:
            return MergeResult(self.machine, reason='machine')
        # Keep the original human baseline even when a model happens to agree.
        # Rebase-to-equality would erase explicit human protection on future updates.
        current = MergeResult(c.text, reason='human_only')
        history = [h for h in self.machine_history if h['revision'] > c.base_revision]
        for h in history:
            candidate = merge3(c.base, c.text, h['text'])
            if candidate.conflict:
                # Preserve previously safe continuations in the visible text as well.
                current = MergeResult(current.text, True, h['text'], candidate.reason)
            else:
                current = candidate
        return current


@dataclass
class Editor:
    id: str
    segment_id: str
    base: str
    base_revision: int
    snapshot: str
    expected_user_seq: int
    draft: str
    draft_revision: int = 0
    state: str = 'open'


@dataclass(frozen=True)
class Note:
    id: str
    segment_id: str
    sample: int
    kind: str
    content: str


class Store:
    """Single-writer, deterministic semantics. Data here is in memory, not durable."""
    def __init__(self) -> None:
        self.worker_epoch = 0
        self.recording = True  # Simulation flag, never an actual device recording state.
        self.captured_samples = 0
        self.segments: dict[str, Segment] = {}
        self.editors: dict[str, Editor] = {}
        self.notes: dict[str, Note] = {}
        self.command_results: dict[str, dict[str, Any]] = {}
        self.command_payloads: dict[str, tuple[str, str]] = {}

    def add_segment(self, sid: str, start: int, end: int) -> Segment:
        if sid in self.segments or start < 0 or end < start:
            raise DomainError('INVALID_SEGMENT')
        s = Segment(sid, start, end)
        self.segments[sid] = s
        return s

    def ingest_simulated_frames(self, count: int) -> None:
        if count < 0:
            raise DomainError('INVALID_FRAME_COUNT')
        if self.recording:
            self.captured_samples += count

    def restart_worker(self) -> int:
        self.worker_epoch += 1
        return self.worker_epoch

    def hypothesis(self, sid: str, revision: int, text: str, *, epoch: int = 0,
                   final: bool = False, end_sample: int | None = None) -> bool:
        s = self.segments[sid]
        if epoch != self.worker_epoch or revision <= s.machine_revision or s.final:
            return False
        if end_sample is not None and end_sample < s.end_sample:
            raise DomainError('AUDIO_RANGE_SHRANK')
        if end_sample is not None:
            s.end_sample = end_sample
        s.machine, s.machine_revision, s.final = text, revision, final
        s.machine_history.append({'revision': revision, 'epoch': epoch, 'text': text, 'final': final})
        return True

    def begin_edit(self, sid: str) -> Editor:
        s = self.segments[sid]
        view = s.projection()
        if s.correction:
            # Conservative reference: keep a cumulative original baseline on re-edit.
            # Production preserves explicit protected ranges with richer provenance.
            base, base_rev = s.correction.base, s.correction.base_revision
        else:
            base, base_rev = s.machine, s.machine_revision
        e = Editor(str(uuid4()), sid, base, base_rev, view.text, s.user_seq, view.text)
        self.editors[e.id] = e
        return e

    def save_draft(self, editor_id: str, text: str, revision: int) -> bool:
        e = self.editors[editor_id]
        if e.state != 'open':
            raise DomainError('EDITOR_CLOSED')
        if revision < e.draft_revision:
            return False
        if revision == e.draft_revision:
            if text != e.draft:
                raise DomainError('DRAFT_REVISION_REUSED')
            return True
        e.draft, e.draft_revision = text, revision
        return True

    def close_editor(self, editor_id: str) -> None:
        e = self.editors[editor_id]
        if e.state == 'open':
            e.state = 'closed'

    def resume_editor(self, editor_id: str) -> Editor:
        e = self.editors[editor_id]
        if e.state != 'closed':
            raise DomainError('EDITOR_NOT_RESUMABLE')
        e.state = 'open'
        return e

    def discard_draft(self, editor_id: str) -> None:
        e = self.editors[editor_id]
        if e.state == 'committed':
            raise DomainError('ALREADY_COMMITTED')
        e.draft, e.state = '', 'discarded'

    def commit(self, editor_id: str, text: str, command_id: str) -> dict[str, Any]:
        payload = (editor_id, text)
        if command_id in self.command_results:
            if self.command_payloads[command_id] != payload:
                raise DomainError('COMMAND_REUSED_WITH_DIFFERENT_PAYLOAD')
            return deepcopy(self.command_results[command_id])
        e = self.editors[editor_id]
        if e.state != 'open':
            raise DomainError('EDITOR_CLOSED')
        s = self.segments[e.segment_id]
        if e.expected_user_seq != s.user_seq:
            e.draft = text  # Preserve the user's attempted submission for recovery.
            raise DomainError('HUMAN_VERSION_CONFLICT')
        if text != e.snapshot:
            s.undo_stack.append(s.correction)
            s.user_seq += 1
            s.correction = Correction(e.base, e.base_revision, text, s.user_seq)
        e.draft, e.state = text, 'committed'
        view = s.projection()
        result = {'segment_id': s.id, 'user_seq': s.user_seq, 'projection': asdict(view)}
        self.command_payloads[command_id] = payload
        self.command_results[command_id] = deepcopy(result)
        return result

    def undo(self, sid: str) -> MergeResult:
        s = self.segments[sid]
        if not s.undo_stack:
            raise DomainError('NOTHING_TO_UNDO')
        s.correction = s.undo_stack.pop()
        s.user_seq += 1
        return s.projection()

    def resolve(self, sid: str, action: str, expected_machine_revision: int,
                expected_user_seq: int, text: str | None = None) -> MergeResult:
        s = self.segments[sid]
        if s.machine_revision != expected_machine_revision or s.user_seq != expected_user_seq:
            raise DomainError('RESOLUTION_VERSION_CONFLICT')
        if action not in ('keep_human', 'accept_machine', 'manual'):
            raise DomainError('INVALID_RESOLUTION')
        if action == 'manual' and text is None:
            raise DomainError('MISSING_RESOLUTION_TEXT')
        human = s.projection().text
        s.undo_stack.append(s.correction)
        s.user_seq += 1
        s.correction = None if action == 'accept_machine' else Correction(
            s.machine, s.machine_revision, human if action == 'keep_human' else str(text), s.user_seq)
        return s.projection()

    def add_note(self, sid: str, sample: int, kind: str, content: str) -> Note:
        s = self.segments[sid]
        if not s.start_sample <= sample <= s.end_sample or kind not in ('note', 'formula', 'image', 'example'):
            raise DomainError('INVALID_NOTE')
        n = Note(str(uuid4()), sid, sample, kind, content)
        self.notes[n.id] = n
        return n

    def dumps(self) -> str:
        """For deterministic restart tests only; production uses SQLite transactions."""
        return json.dumps({'schema': 1, 'worker_epoch': self.worker_epoch,
            'recording': self.recording, 'captured_samples': self.captured_samples,
            'segments': {k: asdict(v) for k, v in self.segments.items()},
            'editors': {k: asdict(v) for k, v in self.editors.items()},
            'notes': {k: asdict(v) for k, v in self.notes.items()},
            'command_results': self.command_results, 'command_payloads': self.command_payloads},
            ensure_ascii=False)

    @classmethod
    def loads(cls, text: str) -> Store:
        obj = json.loads(text)
        if obj.get('schema') != 1:
            raise DomainError('UNSUPPORTED_SNAPSHOT')
        store = cls()
        store.worker_epoch = obj['worker_epoch']
        # A restored simulation does not silently restart a recorder.
        store.recording = False
        store.captured_samples = obj['captured_samples']
        for key, d in obj['segments'].items():
            d['correction'] = Correction(**d['correction']) if d['correction'] else None
            d['undo_stack'] = [Correction(**x) if x else None for x in d['undo_stack']]
            store.segments[key] = Segment(**d)
        store.editors = {k: Editor(**v) for k, v in obj['editors'].items()}
        store.notes = {k: Note(**v) for k, v in obj['notes'].items()}
        store.command_results = obj['command_results']
        store.command_payloads = {k: tuple(v) for k, v in obj['command_payloads'].items()}
        return store
