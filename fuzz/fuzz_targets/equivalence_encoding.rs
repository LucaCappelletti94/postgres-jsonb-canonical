//! Comparison and byte identity agree, and comparison is an equivalence relation.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg18};
use shared::Shape;

/// Well inside `MAX_DEPTH`, so the targets spend their budget on documents rather than on
/// the nesting error.
const BUDGET: usize = 24;

fuzz_target!(|shapes: (Shape, Shape)| {
    let (left, right) = (shapes.0.build(BUDGET), shapes.1.build(BUDGET));
    let (Ok(same), Ok(left_bytes), Ok(right_bytes)) = (
        equivalent::<Pg18>(&left, &right),
        encode::<Pg18>(&left),
        encode::<Pg18>(&right),
    ) else {
        // Acceptance must agree: if one refuses, all three must.
        assert!(
            equivalent::<Pg18>(&left, &right).is_err()
                || encode::<Pg18>(&left).is_err()
                || encode::<Pg18>(&right).is_err()
        );
        return;
    };
    assert_eq!(
        same,
        left_bytes == right_bytes,
        "equality and bytes disagree"
    );
    assert!(equivalent::<Pg18>(&left, &left).expect("reflexive"));
    assert_eq!(same, equivalent::<Pg18>(&right, &left).expect("symmetric"));
});
