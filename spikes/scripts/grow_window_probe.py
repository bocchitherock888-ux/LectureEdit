#!/usr/bin/env python3
"""Sequential growing-window probe against a local llama-server.

Sends complete WAV prefixes (not a native stream). Measures wall time and
effective RTF including recomputation. Transcripts are omitted unless --show-text.
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from scripts.smoke_qwen import NoRedirect, validate_url  # noqa: E402
import base64
import hashlib
import urllib.request
import wave


def send(server: str, key: str, model: str, wav_path: Path) -> dict:
    with wave.open(str(wav_path), 'rb') as wav:
        duration = wav.getnframes() / wav.getframerate()
        if wav.getnchannels() != 1 or wav.getsampwidth() != 2 or wav.getframerate() != 16000:
            raise ValueError(f'{wav_path} is not 16 kHz mono 16-bit PCM')
    raw = wav_path.read_bytes()
    payload = {
        'model': model,
        'messages': [{'role': 'user', 'content': [
            {'type': 'input_audio', 'input_audio': {
                'data': base64.b64encode(raw).decode('ascii'), 'format': 'wav'}}]}],
        'stream': False, 'temperature': 0, 'max_tokens': 2048,
    }
    req = urllib.request.Request(
        server + '/v1/chat/completions',
        data=json.dumps(payload).encode('utf-8'), method='POST',
        headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    started = time.perf_counter()
    with opener.open(req, timeout=300) as response:
        data = response.read(1_048_577)
        if len(data) > 1_048_576:
            raise ValueError('Response exceeded the probe limit')
    elapsed = time.perf_counter() - started
    obj = json.loads(data)
    text = obj['choices'][0]['message']['content']
    if not isinstance(text, str):
        raise ValueError('Unexpected response content type')
    return {
        'wav': wav_path.name,
        'audio_seconds': duration,
        'audio_sha256': hashlib.sha256(raw).hexdigest(),
        'request_wall_seconds': elapsed,
        'single_request_rtf': elapsed / duration if duration else None,
        'response_characters': len(text),
        'transcript': text,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True)
    parser.add_argument('--api-key-file', required=True, type=Path)
    parser.add_argument('--model', required=True)
    parser.add_argument('--wavs', nargs='+', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--show-text', action='store_true')
    args = parser.parse_args()
    server = validate_url(args.server)
    key = args.api_key_file.read_text(encoding='utf-8').strip()
    rows = []
    wall_sum = 0.0
    audio_last = 0.0
    for path in args.wavs:
        row = send(server, key, args.model, path)
        wall_sum += row['request_wall_seconds']
        audio_last = row['audio_seconds']
        if not args.show_text:
            row = {k: v for k, v in row.items() if k != 'transcript'}
        rows.append(row)
        print(json.dumps({k: v for k, v in row.items() if k != 'transcript'}, ensure_ascii=False))
    result = {
        'status': 'response_received',
        'mode': 'growing_window_prefixes',
        'requests': rows,
        'cumulative_request_wall_seconds': wall_sum,
        'final_prefix_audio_seconds': audio_last,
        'effective_rtf_including_recompute': wall_sum / audio_last if audio_last else None,
        'note': 'Each request is a full prefix snapshot. Effective RTF includes repeated work.',
    }
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({k: v for k, v in result.items() if k != 'requests'}, ensure_ascii=False, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
