# G0 spikes

This directory is the G0 working surface. Native checkouts and model files
live in `~/.cache/lectureedit/` so the design-pack Python tests keep scanning
only the small in-tree JSON.

## What this round did

- Recorded this MacBook Air (M4, 16 GB) and the developer toolchain.
- Re-ran the design-pack reference tests on this machine.
- Recorded exact GGUF names, Hugging Face commit, byte sizes and SHA-256.
- Chose a llama.cpp freeze *candidate* after the Qwen3-ASR transcription fix.
- Wrote download and build scripts.
- Ported `merge3` to a Rust spike so G1 has a tested starting point.
- Left Windows, microphone, system-audio, authorised-speech accuracy and
  Computer-use UI verification as `not_run`.

## Do not start here

- Do not open a microphone permission dialog.
- Do not transcribe a user's lecture, Doubao clip, or any other personal file
  unless the user has explicitly authorised that file for G0.
- Do not declare a default backend before a real speech clip is measured.
- Do not scaffold the polished Tauri UI yet. G1 starts after G0 measurements.

## Commands

```bash
# Design-pack reference (always safe)
python3 -m unittest discover -s tests -v
python3 scripts/demo_core.py

# G0 cache (models, vendor, audio)
echo ~/.cache/lectureedit

# Download Q8 decoder + matching mmproj (about 1.019 GB decimal)
bash spikes/scripts/download_qwen_gguf.sh

# Build frozen llama.cpp (Metal) and whisper.cpp
bash spikes/scripts/build_llama_cpp.sh
bash spikes/scripts/build_whisper_cpp.sh

# Rust merge spike
cd spikes/domain-rs && cargo test
```
