# merge3 Rust spike

Port of `reference/core.py:merge3` for G1. Conservative token patches, same
CJK-first tokenizer, overlapping or ambiguous diffs keep the full human text
and the complete machine candidate.

```bash
cargo test
```

This is not the production domain actor. Grapheme ranges, IME, SQLite
transactions and event envelopes remain G1 work.
