//! Key insertion order never reaches the bytes.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg18};
use shared::Shape;

/// Well inside `MAX_DEPTH`, so the targets spend their budget on documents rather than on
/// the nesting error.
const BUDGET: usize = 24;

fuzz_target!(|shape: Shape| {
    let forward = shape.build(BUDGET);
    let reversed = shape.build_reversed(BUDGET);
    let Ok(expected) = encode::<Pg18>(&forward) else {
        return;
    };
    let actual = encode::<Pg18>(&reversed).expect("the same value must still encode");
    assert_eq!(actual, expected, "key order reached the bytes");
    assert!(equivalent::<Pg18>(&forward, &reversed).expect("both accepted"));
});
