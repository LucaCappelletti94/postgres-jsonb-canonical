//! Rewriting a number without changing its value never changes the bytes, and the crate
//! accepts exactly the spellings PostgreSQL's three rules say it should.
//!
//! The acceptance half matters more than it looks. The rules are the only part of this
//! crate with real arithmetic in it, and until the exponent generator was widened they
//! could not be reached by fuzzing at all: two of the three ceilings sat beyond anything
//! the generator could emit.

#![no_main]

#[path = "shared.rs"]
mod shared;

use libfuzzer_sys::fuzz_target;
use postgres_jsonb_canonical::{encode, equivalent, Pg14, Pg18};
use shared::{number, Spelling};

/// Rule 1 for PostgreSQL 16 and later. 14 and 15 stop one lower.
const MAX_EXPONENT: i128 = 1_073_741_823;
/// Rule 2, the display scale.
const MAX_SCALE: i128 = 16_383;
/// Rule 3, the integer digit count.
const MAX_INTEGER_DIGITS: i128 = 131_072;

/// Whether PostgreSQL accepts a spelling, worked out from the three documented rules.
///
/// Written from the rules rather than from the crate, and deliberately in a different
/// shape: it scans the rendered text, works in `i128` so no intermediate can overflow, and
/// shares no code with `src/number.rs`. That is what makes it an oracle rather than an
/// echo.
fn postgres_accepts(spelling: &str, max_exponent: i128) -> bool {
    let body = spelling.strip_prefix('-').unwrap_or(spelling);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let Ok(exponent) = exponent.parse::<i128>() else {
                // Too long to be a machine integer, so far outside every bound.
                return false;
            };
            (mantissa, exponent)
        }
        None => (body, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));

    let fraction_len = i128::try_from(fraction.len()).expect("a string length fits i128");

    // Rule 1: the written exponent.
    if exponent.abs() > max_exponent {
        return false;
    }
    // Rule 2: the display scale, which applies whatever the mantissa is.
    if fraction_len - exponent > MAX_SCALE {
        return false;
    }

    // Rule 3 needs the significant digits, and does not apply to zero.
    let digits = [integer, fraction].concat();
    let leading = digits.len() - digits.trim_start_matches('0').len();
    let trailing = digits.len() - digits.trim_end_matches('0').len();
    if leading + trailing >= digits.len() {
        return true;
    }
    let significant =
        i128::try_from(digits.len() - leading - trailing).expect("a string length fits i128");
    let trailing = i128::try_from(trailing).expect("a string length fits i128");
    significant + (exponent - fraction_len + trailing) <= MAX_INTEGER_DIGITS
}

fuzz_target!(|spelling: Spelling| {
    let rendered = spelling.render();
    let original = number(&rendered);

    // The crate's acceptance must match the rules, on both sides of the one boundary the
    // supported majors disagree about.
    let accepted = encode::<Pg18>(&original).is_ok();
    assert_eq!(
        accepted,
        postgres_accepts(&rendered, MAX_EXPONENT),
        "acceptance disagreed with the rules for {rendered}"
    );
    assert_eq!(
        encode::<Pg14>(&original).is_ok(),
        postgres_accepts(&rendered, MAX_EXPONENT - 1),
        "acceptance disagreed with the rules for {rendered} on PostgreSQL 14"
    );

    let Ok(expected) = encode::<Pg18>(&original) else {
        return;
    };

    for variant in spelling.respellings() {
        let parsed = number(&variant);
        // A respelling can push the display scale out of range even though the value is
        // unchanged, and that refusal is correct.
        let Ok(actual) = encode::<Pg18>(&parsed) else {
            assert!(
                !postgres_accepts(&variant, MAX_EXPONENT),
                "{variant} was refused but the rules accept it"
            );
            continue;
        };
        assert_eq!(
            actual, expected,
            "a respelling changed the bytes: {variant}"
        );
        assert!(equivalent::<Pg18>(&original, &parsed).expect("both accepted"));
    }

    // The older majors accept a strict subset, so anything they take, the newer ones take.
    if let Ok(older) = encode::<Pg14>(&original) {
        assert_eq!(older, expected);
    }
});
