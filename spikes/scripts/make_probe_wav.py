#!/usr/bin/env python3
"""Write a 16 kHz mono 16-bit PCM WAV that is NOT speech.

Use this only to check that a local server accepts the file shape expected by
scripts/smoke_qwen.py. It is not an accuracy fixture and must not be used as
evidence of transcription quality.
"""
from __future__ import annotations

import argparse
import math
import wave
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seconds', type=float, default=2.0)
    args = parser.parse_args()
    rate = 16000
    n = int(rate * args.seconds)
    if not 1 <= n <= rate * 60:
        raise SystemExit('keep the probe between 1 frame and 60 seconds')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(args.output), 'wb') as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        frames = bytearray()
        for i in range(n):
            sample = int(8000 * math.sin(2 * math.pi * 440 * i / rate))
            frames += sample.to_bytes(2, 'little', signed=True)
        wav.writeframes(bytes(frames))
    print(args.output)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
