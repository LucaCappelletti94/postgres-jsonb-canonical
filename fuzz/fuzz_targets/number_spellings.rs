//! Rewriting a number without changing its value never changes the bytes.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg14, Pg18};

use shared::{number, Spelling};

fuzz_target!(|spelling: Spelling| {
    let original = number(&spelling.render());
    let Ok(expected) = encode::<Pg18>(&original) else {
        return;
    };

    for variant in spelling.respellings() {
        let variant = number(&variant);
        // A respelling can push the display scale out of range even though the value is
        // unchanged, and that refusal is correct.
        let Ok(actual) = encode::<Pg18>(&variant) else {
            continue;
        };
        assert_eq!(actual, expected, "a respelling changed the bytes");
        assert!(equivalent::<Pg18>(&original, &variant).expect("both accepted"));
    }

    // The older majors accept a strict subset, so anything they take, the newer ones take.
    if encode::<Pg14>(&original).is_ok() {
        assert_eq!(encode::<Pg14>(&original).ok(), Some(expected));
    }
});
