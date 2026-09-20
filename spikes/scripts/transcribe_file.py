#!/usr/bin/env python3
"""Chunk a local 16 kHz mono WAV and transcribe it on loopback llama-server.

Does not start the server. Chunks are independent snapshots (not native streaming).
"""
from __future__ import annotations

import argparse
import json
import sys
import tempfile
import time
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from parse_qwen import parse_qwen_asr  # noqa: E402
from grow_window_probe import send  # noqa: E402
from scripts.smoke_qwen import validate_url  # noqa: E402


def split_wav(src: Path, chunk_seconds: float, dest: Path) -> list[tuple[Path, float, float]]:
    dest.mkdir(parents=True, exist_ok=True)
    with wave.open(str(src), 'rb') as wav:
        if wav.getnchannels() != 1 or wav.getsampwidth() != 2 or wav.getframerate() != 16000:
            raise ValueError('need 16 kHz mono 16-bit PCM WAV')
        rate = wav.getframerate()
        total = wav.getnframes()
        raw = wav.readframes(total)
    hop = int(chunk_seconds * rate)
    parts = []
    index = 0
    start = 0
    while start < total:
        end = min(start + hop, total)
        chunk = raw[start * 2:end * 2]
        path = dest / f'chunk-{index:04d}.wav'
        with wave.open(str(path), 'wb') as out:
            out.setnchannels(1)
            out.setsampwidth(2)
            out.setframerate(rate)
            out.writeframes(chunk)
        parts.append((path, start / rate, end / rate))
        index += 1
        start = end
    return parts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True)
    parser.add_argument('--api-key-file', required=True, type=Path)
    parser.add_argument('--model', required=True)
    parser.add_argument('--wav', required=True, type=Path)
    parser.add_argument('--chunk-seconds', type=float, default=30.0)
    parser.add_argument('--output-jsonl', type=Path, required=True)
    parser.add_argument('--output-md', type=Path, required=True)
    parser.add_argument('--chunk-dir', type=Path)
    args = parser.parse_args()
    if not 1 <= args.chunk_seconds <= 60:
        raise SystemExit('chunk-seconds must be between 1 and 60')
    validate_url(args.server)
    key = args.api_key_file.read_text(encoding='utf-8').strip()
    tmp = None
    chunk_dir = args.chunk_dir
    if chunk_dir is None:
        tmp = tempfile.TemporaryDirectory()
        chunk_dir = Path(tmp.name)
    parts = split_wav(args.wav, args.chunk_seconds, chunk_dir)
    args.output_jsonl.parent.mkdir(parents=True, exist_ok=True)
    args.output_md.parent.mkdir(parents=True, exist_ok=True)
    rows = []
    started = time.perf_counter()
    with args.output_jsonl.open('w', encoding='utf-8') as jsonl:
        for path, t0, t1 in parts:
            result = send(args.server, key, args.model, path)
            parsed = parse_qwen_asr(result['transcript'])
            row = {
                'start_seconds': t0,
                'end_seconds': t1,
                'audio_seconds': result['audio_seconds'],
                'audio_sha256': result['audio_sha256'],
                'request_wall_seconds': result['request_wall_seconds'],
                'single_request_rtf': result['single_request_rtf'],
                'language': parsed['language'],
                'text': parsed['text'],
            }
            jsonl.write(json.dumps(row, ensure_ascii=False) + '\n')
            jsonl.flush()
            rows.append(row)
            print(f'{t0:7.1f}-{t1:7.1f}s  rtf={result["single_request_rtf"]:.3f}  {parsed["text"][:80]!r}', flush=True)
    elapsed = time.perf_counter() - started
    audio_total = rows[-1]['end_seconds'] if rows else 0.0
    lines = [
        f'# Transcript of `{args.wav.name}`',
        '',
        f'- engine: llama.cpp Qwen3-ASR via `{args.server}`',
        f'- chunk: {args.chunk_seconds} s independent snapshots',
        f'- audio: {audio_total:.3f} s',
        f'- wall: {elapsed:.3f} s',
        f'- effective RTF: {elapsed / audio_total if audio_total else None}',
        f'- chunks: {len(rows)}',
        '',
    ]
    for row in rows:
        lines.append(f'## {row["start_seconds"]:.1f}–{row["end_seconds"]:.1f} s')
        lines.append('')
        lines.append(row['text'] or '_(empty)_')
        lines.append('')
    args.output_md.write_text('\n'.join(lines), encoding='utf-8')
    summary = {
        'status': 'done',
        'chunks': len(rows),
        'audio_seconds': audio_total,
        'wall_seconds': elapsed,
        'effective_rtf': elapsed / audio_total if audio_total else None,
        'markdown': str(args.output_md),
        'jsonl': str(args.output_jsonl),
    }
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    if tmp is not None:
        tmp.cleanup()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
