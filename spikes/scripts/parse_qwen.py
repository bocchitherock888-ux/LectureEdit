"""Parse llama.cpp Qwen3-ASR chat output into language + transcript text."""
from __future__ import annotations
import re

_TAG = re.compile(
    r'^(?:language\s+([^\n<]+))?\s*<asr_text>(.*)\Z',
    re.DOTALL | re.IGNORECASE,
)


def parse_qwen_asr(raw: str) -> dict[str, str | None]:
    if not isinstance(raw, str):
        raise TypeError('parse_qwen_asr expects a string')
    text = raw.strip()
    match = _TAG.search(text)
    if match:
        lang = match.group(1)
        body = match.group(2)
        return {
            'language': lang.strip() if lang else None,
            'text': body.strip(),
            'raw': raw,
        }
    return {'language': None, 'text': text, 'raw': raw}
