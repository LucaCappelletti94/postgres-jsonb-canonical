//! The PostgreSQL oracle corpus: what it contains, and how it is stored.
//!
//! A live server is far too slow to sit inside a coverage-guided fuzzing loop, and too
//! slow to consult per assertion. So the server is asked once, offline, and its answers are
//! written to a file that everything else replays. `tests/oracle.rs` replays it with no
//! Docker at all, `tests/differential.rs` re-verifies it against live servers, and
//! `fuzz/fuzz_targets/postgres_oracle.rs` lets the fuzzer explore the pairs.
//!
//! Two things are recorded per spelling: which majors accept it, and which equivalence
//! class the server put it in. The class is the valuable part, because it makes every one
//! of the N-squared pairs checkable from N recorded rows.

// Compiled separately into every test binary that declares `mod common`, and each of them
// uses a different part: `oracle.rs` replays the file, `differential.rs` records and
// verifies it. Neither uses all of it.
#![allow(dead_code)]

use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

/// Majors the corpus records, newest last. The newest is the most permissive, so it is the
/// one that assigns equivalence classes.
pub const MAJORS: [&str; 5] = ["14", "15", "16", "17", "18"];

/// Where the recorded answers live, relative to the crate root.
pub fn oracle_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("oracle/postgres-jsonb.tsv")
}

/// One spelling, as the servers answered for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub spelling: String,
    /// Majors that accepted it, in the order of [`MAJORS`].
    pub accepted_by: Vec<String>,
    /// Equivalence class under `jsonb =`, or `None` where no server accepted it.
    pub group: Option<u32>,
}

impl Recorded {
    pub fn accepted_by_major(&self, major: &str) -> bool {
        self.accepted_by.iter().any(|accepted| accepted == major)
    }
}

/// Renders the corpus in the committed format: a comment header, then one row per
/// spelling, tab separated, in corpus order so a regeneration produces a reviewable diff.
pub fn render(rows: &[Recorded]) -> String {
    let mut out = String::new();
    out.push_str("# PostgreSQL oracle for postgres-jsonb-canonical.\n");
    out.push_str("# Recorded from live servers. Regenerate with:\n");
    out.push_str("#   UPDATE_ORACLE=1 cargo test --release --test differential\n");
    out.push_str("# Columns: spelling, majors accepting it, equivalence class under `jsonb =`.\n");
    out.push_str("# A class of `-` means every server refused the spelling.\n");
    for row in rows {
        let accepted = if row.accepted_by.is_empty() {
            "none".to_owned()
        } else {
            row.accepted_by.join(",")
        };
        let group = row
            .group
            .map_or_else(|| "-".to_owned(), |group| group.to_string());
        writeln!(out, "{}\t{accepted}\t{group}", row.spelling).expect("writing to a String");
    }
    out
}

/// Parses the committed format back.
pub fn parse(text: &str) -> Vec<Recorded> {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split('\t');
            let spelling = columns.next().expect("a row has a spelling").to_owned();
            let accepted = columns.next().expect("a row has an acceptance column");
            let group = columns.next().expect("a row has a class column");
            Recorded {
                spelling,
                accepted_by: if accepted == "none" {
                    Vec::new()
                } else {
                    accepted.split(',').map(ToOwned::to_owned).collect()
                },
                group: group.parse().ok(),
            }
        })
        .collect()
}

/// The spellings the servers are asked about.
///
/// Built mechanically rather than listed by hand, and deliberately kept short: the
/// interesting arithmetic sits at the ceilings, and every ceiling is reachable with a brief
/// spelling. `1e131072` exceeds the integer-digit ceiling in eight characters. Long digit
/// runs are covered by `tests/contract.rs` instead, and would make this file unreviewable.
pub fn corpus() -> Vec<String> {
    let mut spellings = Vec::new();

    // Every ceiling, approached from both sides, on a bare mantissa and on one carrying a
    // fraction digit, since the fraction shifts the display scale by one.
    for centre in [
        1_073_741_823_i64, // rule 1, the written exponent
        -1_073_741_823,
        -16_383, // rule 2, the display scale
        16_383,
        131_071, // rule 3, the integer digits
        131_072,
        -131_072,
    ] {
        for offset in -2_i64..=2 {
            let exponent = centre + offset;
            spellings.push(format!("0e{exponent}"));
            spellings.push(format!("1e{exponent}"));
            spellings.push(format!("1.5e{exponent}"));
            spellings.push(format!("0.1e{exponent}"));
            spellings.push(format!("-1e{exponent}"));
        }
    }

    // Values with several spellings, so the recorded classes have real structure rather
    // than being one spelling each.
    for (mantissa, exponents) in [
        ("1", [0_i64, 1, 2, -1, -2]),
        ("1.5", [0, 1, 2, -1, -2]),
        ("0", [0, 1, 2, -1, -2]),
        ("9", [0, 3, 6, -3, -6]),
        ("1.25", [0, 2, 4, -2, -4]),
        ("123456789", [0, 5, 10, -5, -10]),
    ] {
        for exponent in exponents {
            spellings.push(format!("{mantissa}e{exponent}"));
            spellings.push(format!("-{mantissa}e{exponent}"));
            spellings.push(format!("{mantissa}0e{}", exponent - 1));
            spellings.push(format!("{mantissa}00e{}", exponent - 2));
        }
    }

    // Trailing and leading zeros, which must not change the value or the class.
    for spelling in [
        "0", "-0", "0.0", "-0.0", "0.000", "0e0", "-0e0", "1", "1.0", "1.00", "1.000", "1e0",
        "1E0", "1e+0", "1e-0", "0.1", "0.10", "0.100", "1e-1", "10e-2", "100e-3", "10", "1e1",
        "0.1e2", "0.01e3", "100", "1e2", "1.0e2", "0.1e3", "1.5e2", "150", "15.0e1", "1500e-1",
        "1.55e1", "15.5", "0.155e2", "1550e-2",
    ] {
        spellings.push((*spelling).to_owned());
    }

    // Precision an f64 cannot hold, so a lossy path would collapse these together.
    for spelling in [
        "12345678901234567890123456",
        "12345678901234567890123457",
        "1.2345678901234567890123456e25",
        "0.1000000000000000000000001",
        "0.1000000000000000000000002",
        "1.0000000000000000000000001",
    ] {
        spellings.push((*spelling).to_owned());
    }

    // Exponent spelling oddities the JSON grammar allows.
    for spelling in [
        "1e05",
        "1e+5",
        "1E5",
        "1E+5",
        "1e-05",
        "1e0000000000000000005",
    ] {
        spellings.push((*spelling).to_owned());
    }

    // JSON forbids a leading zero on a multi-digit integer part, so the point-shifting
    // above can emit spellings like `00e0`. PostgreSQL rejects those for the same reason,
    // but more to the point `serde_json` cannot produce such a `Value`, so they sit
    // outside the crate's input domain entirely and would only be noise in the file.
    spellings.retain(|spelling| serde_json::from_str::<serde_json::Value>(spelling).is_ok());

    spellings.sort_unstable();
    spellings.dedup();
    spellings
}

/// Assigns dense class numbers from a map of spelling to server-chosen representative.
pub fn dense_classes(representatives: &BTreeMap<String, i32>) -> BTreeMap<String, u32> {
    let mut seen: BTreeMap<i32, u32> = BTreeMap::new();
    let mut classes = BTreeMap::new();
    for (spelling, representative) in representatives {
        let next = u32::try_from(seen.len()).expect("the corpus is small");
        let class = *seen.entry(*representative).or_insert(next);
        classes.insert(spelling.clone(), class);
    }
    classes
}
