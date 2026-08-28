//! Replays real PostgreSQL answers, and mutates around them.
//!
//! A live server cannot sit inside a coverage-guided loop, so the servers were asked once
//! and their answers committed to `oracle/postgres-jsonb.tsv`. The recording is compiled
//! in here.
//!
//! `tests/oracle.rs` already checks the crate against every recorded row, which a fuzzer
//! would cover in seconds and then add nothing. What this target adds is the part a fixed
//! test cannot do: it takes a spelling a server verified, rewrites it in a way that cannot
//! change its value, and asserts the rewrite lands in the same equivalence class. The
//! server never saw the rewrite, but its verdict on the anchor still binds it.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg14, Pg18};
use serde_json::Value;
use shared::respellings_of;

/// The committed recording, parsed once per process.
///
/// The parser is a few lines and is duplicated from `tests/common/mod.rs` rather than
/// shared, because the fuzz targets are a separate workspace and a dependency edge between
/// them would be worse than these six lines.
struct Row {
    spelling: &'static str,
    accepted_by_oldest: bool,
    accepted_by_newest: bool,
    class: Option<u32>,
}

fn oracle() -> Vec<Row> {
    include_str!("../../oracle/postgres-jsonb.tsv")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split('\t');
            let spelling = columns.next().expect("a row has a spelling");
            let accepted = columns.next().expect("a row has an acceptance column");
            let class = columns.next().expect("a row has a class column");
            Row {
                spelling,
                accepted_by_oldest: accepted.split(',').any(|major| major == "14"),
                accepted_by_newest: accepted.split(',').any(|major| major == "18"),
                class: class.parse().ok(),
            }
        })
        .collect()
}

fn parse(spelling: &str) -> Option<Value> {
    serde_json::from_str(spelling).ok()
}

fuzz_target!(|input: (u16, u16, u8)| {
    let rows = oracle();
    assert!(!rows.is_empty(), "the oracle is empty");

    let (left_at, right_at, variant) = input;
    let left = &rows[usize::from(left_at) % rows.len()];
    let right = &rows[usize::from(right_at) % rows.len()];

    let Some(left_value) = parse(left.spelling) else {
        return;
    };
    let Some(right_value) = parse(right.spelling) else {
        return;
    };

    // The recording binds acceptance on both sides of the one boundary the majors differ
    // about.
    assert_eq!(
        encode::<Pg18>(&left_value).is_ok(),
        left.accepted_by_newest,
        "acceptance drifted for {}",
        left.spelling
    );
    assert_eq!(
        encode::<Pg14>(&left_value).is_ok(),
        left.accepted_by_oldest,
        "acceptance on PostgreSQL 14 drifted for {}",
        left.spelling
    );

    // And it binds the relation between any two rows, which is why the class was recorded
    // rather than the pairwise answers: N rows carry all N-squared verdicts.
    if let (Some(left_class), Some(right_class)) = (left.class, right.class) {
        let same = left_class == right_class;
        assert_eq!(
            equivalent::<Pg18>(&left_value, &right_value).expect("classified rows are in range"),
            same,
            "`{}` versus `{}`",
            left.spelling,
            right.spelling
        );
    }

    // The part a fixed replay cannot reach: rewrite a verified spelling without changing
    // its value, and hold it to the anchor's class.
    let Ok(anchor) = encode::<Pg18>(&left_value) else {
        return;
    };
    let variants = respellings_of(left.spelling);
    if variants.is_empty() {
        return;
    }
    let rewritten = &variants[usize::from(variant) % variants.len()];
    let Some(rewritten_value) = parse(rewritten) else {
        return;
    };
    // A rewrite can push the display scale out of range even though the value is the same,
    // and refusing it is correct.
    let Ok(actual) = encode::<Pg18>(&rewritten_value) else {
        return;
    };
    assert_eq!(
        actual, anchor,
        "rewriting `{}` as `{rewritten}` left the class the server put it in",
        left.spelling
    );
});
