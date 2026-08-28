//! Replays the recorded PostgreSQL answers with no server in the loop.
//!
//! `tests/differential.rs` proves the recording still matches live servers. This file
//! proves the crate matches the recording, and it needs no Docker, so it runs on every
//! push and on targets where a server could never run at all.

// This suite needs dev-dependencies, which are gated on little-endian so the big-endian
// job does not have to build them. See the comment in Cargo.toml.
#![cfg(target_endian = "little")]

use postgres_jsonb_canonical::{encode, equivalent, Pg14, Pg15, Pg16, Pg17, Pg18};
use serde_json::Value;

mod common;

fn recorded() -> Vec<common::Recorded> {
    let text = std::fs::read_to_string(common::oracle_path()).expect("the oracle file exists");
    let rows = common::parse(&text);
    assert!(!rows.is_empty(), "the oracle file is empty");
    rows
}

fn parse(spelling: &str) -> Value {
    serde_json::from_str(spelling).expect("a recorded spelling is valid JSON")
}

/// `encode` for a major named at runtime, so one loop can cover all five markers.
fn accepts(major: &str, value: &Value) -> bool {
    match major {
        "14" => encode::<Pg14>(value).is_ok(),
        "15" => encode::<Pg15>(value).is_ok(),
        "16" => encode::<Pg16>(value).is_ok(),
        "17" => encode::<Pg17>(value).is_ok(),
        "18" => encode::<Pg18>(value).is_ok(),
        other => panic!("no marker for PostgreSQL {other}"),
    }
}

#[test]
fn the_oracle_is_worth_replaying() {
    // Guards the tests below from passing on a corpus that says nothing: they need real
    // refusals, real classes, and at least one spelling the majors disagree about.
    let rows = recorded();
    let refused = rows.iter().filter(|row| row.accepted_by.is_empty()).count();
    let classes: std::collections::BTreeSet<_> = rows.iter().filter_map(|row| row.group).collect();
    let disputed = rows
        .iter()
        .filter(|row| !row.accepted_by.is_empty() && row.accepted_by.len() < common::MAJORS.len())
        .count();

    assert!(rows.len() >= 200, "only {} spellings recorded", rows.len());
    assert!(
        refused >= 50,
        "only {refused} spellings are refused by every server"
    );
    assert!(
        classes.len() >= 50,
        "only {} equivalence classes",
        classes.len()
    );
    assert_eq!(
        disputed, 1,
        "expected exactly one spelling the majors disagree about, found {disputed}"
    );
}

#[test]
fn acceptance_matches_every_server() {
    for row in recorded() {
        let value = parse(&row.spelling);
        for major in common::MAJORS {
            assert_eq!(
                accepts(major, &value),
                row.accepted_by_major(major),
                "pg {major} and the crate disagree about `{}`",
                row.spelling
            );
        }
    }
}

#[test]
fn the_classes_are_exactly_the_canonical_byte_groups() {
    // The server's partition and the crate's partition have to be the same relation, so
    // every pair is checked in both directions: same class implies identical bytes, and
    // different class implies different bytes.
    let rows: Vec<_> = recorded()
        .into_iter()
        .filter(|row| row.group.is_some())
        .collect();
    let encoded: Vec<Vec<u8>> = rows
        .iter()
        .map(|row| encode::<Pg18>(&parse(&row.spelling)).expect("a classified spelling encodes"))
        .collect();

    for (index, left) in rows.iter().enumerate() {
        for (offset, right) in rows[index..].iter().enumerate() {
            let same_class = left.group == right.group;
            assert_eq!(
                encoded[index] == encoded[index + offset],
                same_class,
                "`{}` and `{}`: server says same class is {same_class}",
                left.spelling,
                right.spelling
            );
            assert_eq!(
                equivalent::<Pg18>(&parse(&left.spelling), &parse(&right.spelling))
                    .expect("classified spellings are in range"),
                same_class,
                "`{}` and `{}`: equivalent disagrees with the server",
                left.spelling,
                right.spelling
            );
        }
    }
}

#[test]
fn a_refused_spelling_is_refused_by_the_crate_too() {
    for row in recorded().iter().filter(|row| row.accepted_by.is_empty()) {
        let value = parse(&row.spelling);
        assert!(
            encode::<Pg18>(&value).is_err(),
            "`{}` is refused by every server but the crate took it",
            row.spelling
        );
        assert!(equivalent::<Pg18>(&value, &value).is_err());
    }
}
