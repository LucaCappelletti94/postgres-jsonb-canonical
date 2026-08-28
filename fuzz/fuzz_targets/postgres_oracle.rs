//! Replays recorded PostgreSQL answers, and mutates around them.
//!
//! Beyond what `tests/oracle.rs` already checks: rewrite a server-verified spelling
//! without changing its value, and hold it to the anchor's class.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg14, Pg18};
use serde_json::Value;
use shared::respellings_of;

/// The committed recording; parsed here rather than shared, the fuzz crate being separate.
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

    // Both sides of the one boundary the majors differ about.
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

    // Classes rather than pairwise answers: N rows carry all N-squared verdicts.
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

    // The part a fixed replay cannot reach.
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
    // A rewrite can push the scale out of range; that refusal is correct.
    let Ok(actual) = encode::<Pg18>(&rewritten_value) else {
        return;
    };
    assert_eq!(
        actual, anchor,
        "rewriting `{}` as `{rewritten}` left the class the server put it in",
        left.spelling
    );
});
