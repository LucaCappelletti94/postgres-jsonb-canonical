//! Nesting and width limits return an error rather than overflowing the stack.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg18};
use serde_json::Value;
use shared::Shape;

/// Well inside `MAX_DEPTH`, so the targets spend their budget on documents rather than on
/// the nesting error.
const BUDGET: usize = 24;

fuzz_target!(|input: (u8, Shape)| {
    let (extra, shape) = input;
    let mut value = shape.build(BUDGET);

    // Wrap the value until it is somewhere either side of the depth cap.
    let layers = usize::from(extra) + 96;
    for index in 0..layers {
        value = if index % 2 == 0 {
            Value::Array(vec![value])
        } else {
            let mut level = serde_json::Map::new();
            level.insert("k".to_owned(), value);
            Value::Object(level)
        };
    }

    // Whatever the answer, it is an answer: no panic, no overflow, and the two entry points
    // agree about whether this value is acceptable.
    //
    // Only acceptance is asserted. A value can break several rules at once, and the two
    // functions do not walk objects in the same order, so they may name different reasons
    // for the same refusal. That is documented on `CanonicalError`.
    let encoded = encode::<Pg18>(&value);
    let compared = equivalent::<Pg18>(&value, &value);
    assert_eq!(
        encoded.is_ok(),
        compared.is_ok(),
        "encode and equivalent disagree on acceptance"
    );
});
