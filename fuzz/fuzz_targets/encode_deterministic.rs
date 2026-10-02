//! A value never panics the encoder, and encoding it twice gives the same bytes.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, Pg18};
use shared::Shape;

/// Well inside `MAX_DEPTH`, so the targets spend their budget on documents rather than on
/// the nesting error.
const BUDGET: usize = 24;

fuzz_target!(|shape: Shape| {
    let value = shape.build(BUDGET);
    let Ok(first) = encode::<Pg18>(&value) else {
        return;
    };
    let second = encode::<Pg18>(&value).expect("a value that encoded once must encode again");
    assert_eq!(first, second, "encoding is not deterministic");
    assert!(first.starts_with(&postgres_jsonb_canonical::MAGIC));
});
