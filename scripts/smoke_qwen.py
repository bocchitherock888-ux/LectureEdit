"""Local-only fixed-WAV llama-server smoke probe; run after G0 server preparation.

This script has no model downloader and does not start a server. It sends one complete
WAV, not a growing audio stream. The production Qwen template must be verified in G0.
"""
from __future__ import annotations
import argparse
import base64
import hashlib
import json
from pathlib import Path
import time
import urllib.error
import urllib.parse
import urllib.request
import wave

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('Redirects are disabled for this local-only probe')

def validate_url(url: str) -> str:
    p = urllib.parse.urlparse(url)
    if (p.scheme != 'http' or p.hostname != '127.0.0.1' or not p.port
            or p.username or p.password or p.query or p.fragment or p.path not in ('', '/')):
        raise ValueError('Use an explicit loopback URL: http://127.0.0.1:PORT')
    return url.rstrip('/')

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True, help='http://127.0.0.1:PORT')
    parser.add_argument('--wav', required=True, type=Path, help='Authorised PCM WAV, <= 60 seconds')
    parser.add_argument('--api-key-file', required=True, type=Path, help='Local credential file; never logged')
    parser.add_argument('--model', required=True, help='Alias accepted by the running server')
    parser.add_argument('--output', type=Path, help='Save metadata JSON; transcript only with --show-text')
    parser.add_argument('--show-text', action='store_true')
    args = parser.parse_args()
    try:
        server = validate_url(args.server)
        size = args.wav.stat().st_size
        if not 44 <= size <= 16_000_000:
            raise ValueError('WAV file must be between 44 bytes and 16 MB')
        with wave.open(str(args.wav), 'rb') as wav:
            duration = wav.getnframes() / wav.getframerate()
            if duration <= 0 or duration > 60:
                raise ValueError('Use a WAV between 0 and 60 seconds')
            if wav.getnchannels() != 1 or wav.getsampwidth() != 2 or wav.getframerate() != 16000:
                raise ValueError('Use 16 kHz mono, 16-bit PCM WAV for this probe')
        raw = args.wav.read_bytes()
        key = args.api_key_file.read_text(encoding='utf-8').strip()
        if not key or any(c in key for c in '\r\n'):
            raise ValueError('Credential file must contain one non-empty key')
        payload = {'model': args.model, 'messages': [{'role':'user', 'content':[
            {'type':'input_audio', 'input_audio':{'data':base64.b64encode(raw).decode('ascii'), 'format':'wav'}}
        ]}], 'stream':False, 'temperature':0, 'max_tokens':2048}
        req = urllib.request.Request(server+'/v1/chat/completions',
            data=json.dumps(payload).encode('utf-8'), method='POST',
            headers={'Content-Type':'application/json', 'Authorization':'Bearer '+key})
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
            raise ValueError('Unexpected response content type; inspect the frozen server contract')
        result = {'status':'response_received', 'audio_sha256':hashlib.sha256(raw).hexdigest(),
                  'audio_seconds':duration, 'request_wall_seconds':elapsed,
                  'single_request_rtf':elapsed/duration, 'response_characters':len(text),
                  'note':'Fixed audio request only. Template/accuracy and continuous effective RTF need separate validation.'}
        if args.show_text:
            result['transcript'] = text
        output = json.dumps(result, ensure_ascii=False, indent=2)
        print(output)
        if args.output:
            args.output.write_text(output+'\n', encoding='utf-8')
        return 0
    except urllib.error.HTTPError as exc:
        print(json.dumps({'status':'failed', 'http_status':exc.code,
                          'note':'Check the frozen model template and server contract locally. Response body omitted.'}))
    except (OSError, ValueError, KeyError, IndexError, TypeError, wave.Error) as exc:
        print(json.dumps({'status':'failed', 'error_type':type(exc).__name__, 'message':str(exc)}, ensure_ascii=False))
    return 1

if __name__ == '__main__':
    raise SystemExit(main())
