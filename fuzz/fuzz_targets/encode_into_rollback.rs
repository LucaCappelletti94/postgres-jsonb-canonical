//! A refused value leaves the caller's buffer exactly as it was.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, encode_into, Pg18};
use serde_json::Value;
use shared::Shape;

/// Well inside `MAX_DEPTH`, so the targets spend their budget on documents rather than on
/// the nesting error.
const BUDGET: usize = 24;

fuzz_target!(|input: (Vec<u8>, Shape)| {
    let (prefix, shape) = input;
    let value = shape.build(BUDGET);

    let mut buffer = prefix.clone();
    match encode_into::<Pg18>(&value, &mut buffer) {
        Ok(()) => {
            assert_eq!(&buffer[..prefix.len()], &prefix[..], "prefix was disturbed");
            let standalone = encode::<Pg18>(&value).expect("encoded once already");
            assert_eq!(&buffer[prefix.len()..], standalone.as_slice());
        }
        Err(_) => assert_eq!(buffer, prefix, "buffer not restored after a refusal"),
    }

    // Planting a refused number after a good value must still roll the whole thing back.
    let poisoned = Value::Array(vec![
        value,
        serde_json::from_str("1e-16384").expect("valid JSON, out of PostgreSQL's range"),
    ]);
    let mut buffer = prefix.clone();
    assert!(encode_into::<Pg18>(&poisoned, &mut buffer).is_err());
    assert_eq!(buffer, prefix, "buffer not restored after a nested refusal");
});
