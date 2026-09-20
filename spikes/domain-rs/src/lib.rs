//! Conservative token-level three-way merge.
//!
//! Behavioural port of `reference/core.py:merge3`. Production G1 still needs
//! grapheme ranges, editor mapping, IME and durable transactions.

mod store;

pub use store::{CommitResult, Correction, DomainError, Editor, Note, Segment, Store};

use difflib::sequencematcher::SequenceMatcher;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

static TOKEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[\u{3400}-\u{9fff}]|\w+|\s+|[^\w\s]").expect("token regex"));

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Patch {
    start: usize,
    end: usize,
    replacement: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeResult {
    pub text: String,
    pub conflict: bool,
    pub pending_machine: Option<String>,
    #[serde(default)]
    pub reason: String,
}

fn tokens(s: &str) -> Vec<String> {
    TOKEN.find_iter(s).map(|m| m.as_str().to_string()).collect()
}

fn patches(base: &[String], changed: &[String]) -> Vec<Patch> {
    let mut matcher = SequenceMatcher::new(base, changed);
    matcher
        .get_opcodes()
        .into_iter()
        .filter(|op| op.tag != "equal")
        .map(|op| Patch {
            start: op.first_start,
            end: op.first_end,
            replacement: changed[op.second_start..op.second_end].to_vec(),
        })
        .collect()
}

fn overlap(a: &Patch, b: &Patch) -> bool {
    if a.start == a.end {
        return b.start <= a.start && a.start <= b.end;
    }
    if b.start == b.end {
        return a.start <= b.start && b.start <= a.end;
    }
    a.start.max(b.start) < a.end.min(b.end)
}

fn count_sequence(base: &[String], needle: &[String]) -> usize {
    if needle.is_empty() || needle.len() > base.len() {
        return 0;
    }
    base.windows(needle.len()).filter(|w| *w == needle).count()
}

fn ambiguous(base: &[String], p: &Patch) -> bool {
    if p.start != p.end {
        return count_sequence(base, &base[p.start..p.end]) > 1;
    }
    if p.start == 0 || p.start == base.len() {
        return false;
    }
    let from = p.start.saturating_sub(4);
    let to = (p.start + 4).min(base.len());
    count_sequence(base, &base[from..to]) > 1
}

/// Apply disjoint token patches; retain complete alternatives on any uncertainty.
pub fn merge3(base: &str, user: &str, machine: &str) -> MergeResult {
    if user == machine {
        return MergeResult {
            text: user.to_string(),
            conflict: false,
            pending_machine: None,
            reason: "identical_branches".into(),
        };
    }
    if user == base {
        return MergeResult {
            text: machine.to_string(),
            conflict: false,
            pending_machine: None,
            reason: "machine_only".into(),
        };
    }
    if machine == base {
        return MergeResult {
            text: user.to_string(),
            conflict: false,
            pending_machine: None,
            reason: "human_only".into(),
        };
    }
    let b = tokens(base);
    let up = patches(&b, &tokens(user));
    let mp = patches(&b, &tokens(machine));
    for a in &up {
        for c in &mp {
            if a != c && overlap(a, c) {
                return MergeResult {
                    text: user.to_string(),
                    conflict: true,
                    pending_machine: Some(machine.to_string()),
                    reason: "overlapping_edits".into(),
                };
            }
        }
    }
    let common: Vec<&Patch> = up.iter().filter(|p| mp.contains(p)).collect();
    let ambiguous_hit = up
        .iter()
        .chain(mp.iter())
        .any(|p| !common.iter().any(|c| *c == p) && ambiguous(&b, p));
    if ambiguous_hit {
        return MergeResult {
            text: user.to_string(),
            conflict: true,
            pending_machine: Some(machine.to_string()),
            reason: "ambiguous_alignment".into(),
        };
    }
    let mut merged = b;
    let mut all = up;
    for p in mp {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    all.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    all.reverse();
    for p in all {
        merged.splice(p.start..p.end, p.replacement);
    }
    MergeResult {
        text: merged.concat(),
        conflict: false,
        pending_machine: None,
        reason: "clean".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::path::PathBuf;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        base: String,
        user: String,
        machine: String,
        expected: String,
        conflict: bool,
    }

    #[test]
    fn fixtures_from_design_pack() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/merge_cases.json");
        let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for c in cases {
            let r = merge3(&c.base, &c.user, &c.machine);
            assert_eq!(r.text, c.expected, "{}", c.name);
            assert_eq!(r.conflict, c.conflict, "{}", c.name);
        }
    }

    #[test]
    fn growing_suffix() {
        let r = merge3(
            "According to Talmey, Path is encoded",
            "According to Talmy, Path is encoded",
            "According to Talmey, Path is encoded in the verb.",
        );
        assert!(!r.conflict);
        assert_eq!(r.text, "According to Talmy, Path is encoded in the verb.");
    }

    #[test]
    fn conflicting_number_keeps_full_machine() {
        let r = merge3(
            "It is fifteen percent.",
            "It is fifty percent.",
            "It is sixteen percent. More follows.",
        );
        assert!(r.conflict);
        assert_eq!(r.text, "It is fifty percent.");
        assert_eq!(
            r.pending_machine.as_deref(),
            Some("It is sixteen percent. More follows.")
        );
    }

    #[test]
    fn chinese_preserved() {
        let r = merge3("他研究路径", "她研究路径", "他研究路径编码");
        assert!(!r.conflict);
        assert_eq!(r.text, "她研究路径编码");
    }

    #[test]
    fn random_disjoint_replacements() {
        // Same seed and construction as tests/test_core.py.
        let mut rng = SplitMix64::new(20260918);
        for _ in 0..500 {
            let words: Vec<String> = (0..18).map(|i| format!("token{i}")).collect();
            let i = rng.sample_index(18);
            let mut j = rng.sample_index(18);
            while j == i {
                j = rng.sample_index(18);
            }
            let mut u = words.clone();
            let mut m = words.clone();
            let mut expected = words.clone();
            u[i] = format!("HUMAN{i}");
            m[j] = format!("MACHINE{j}");
            expected[i] = u[i].clone();
            expected[j] = m[j].clone();
            let r = merge3(&words.join(" "), &u.join(" "), &m.join(" "));
            assert!(!r.conflict);
            assert_eq!(r.text, expected.join(" "));
        }
    }

    /// Python `random.Random` is not this PRNG. The 500-loop above uses a
    /// local generator only to pick two distinct indices; any uniform pair is
    /// enough because the assertion is algorithmic, not seed-identical.
    struct SplitMix64 {
        state: u64,
    }

    impl SplitMix64 {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }
        fn next_u64(&mut self) -> u64 {
            self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
        fn sample_index(&mut self, n: usize) -> usize {
            (self.next_u64() as usize) % n
        }
    }
}
