//! The PostgreSQL acceptance and equality contract, stated as examples.
//!
//! `tests/differential.rs` re-derives every one of them from a live server.

use postgres_jsonb_canonical::{
    encode, encode_into, equivalent, CanonicalError, Pg14, Pg15, Pg16, Pg17, Pg18, MAX_DEPTH,
};
use serde_json::{json, Value};

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("test input is valid JSON")
}

fn assert_same(left: &str, right: &str) {
    let (left, right) = (parse(left), parse(right));
    assert!(
        equivalent::<Pg18>(&left, &right).expect("in range"),
        "{left} should equal {right}"
    );
    assert_eq!(
        encode::<Pg18>(&left).expect("in range"),
        encode::<Pg18>(&right).expect("in range"),
        "{left} and {right} should encode alike"
    );
}

fn assert_differs(left: &str, right: &str) {
    let (left, right) = (parse(left), parse(right));
    assert!(
        !equivalent::<Pg18>(&left, &right).expect("in range"),
        "{left} should differ from {right}"
    );
    assert_ne!(
        encode::<Pg18>(&left).expect("in range"),
        encode::<Pg18>(&right).expect("in range"),
        "{left} and {right} should encode differently"
    );
}

fn assert_refused(text: &str) {
    let value = parse(text);
    assert_eq!(
        encode::<Pg18>(&value),
        Err(CanonicalError::NumberOutOfRange),
        "{text} should be refused"
    );
    assert_eq!(
        equivalent::<Pg18>(&value, &value),
        Err(CanonicalError::NumberOutOfRange)
    );
}

fn assert_accepted(text: &str) {
    let value = parse(text);
    assert!(encode::<Pg18>(&value).is_ok(), "{text} should be accepted");
}

/// Builds a value nested `depth` arrays deep.
fn nested(depth: usize) -> Value {
    let mut value = Value::Null;
    for _ in 0..depth {
        value = Value::Array(vec![value]);
    }
    value
}

/// Builds a value nested `depth` objects deep, objects taking a different branch from
/// arrays in both the encoder and the validator.
fn nested_objects(depth: usize) -> Value {
    let mut value = Value::Null;
    for _ in 0..depth {
        let mut level = serde_json::Map::new();
        level.insert("k".to_owned(), value);
        value = Value::Object(level);
    }
    value
}

mod numbers {
    use super::*;

    #[test]
    fn spelling_does_not_change_a_number() {
        assert_same("1", "1.0");
        assert_same("1", "1.00");
        assert_same("1", "1e0");
        assert_same("1", "1E0");
        assert_same("1", "1e+0");
        assert_same("100", "1e2");
        assert_same("100", "1.0e2");
        assert_same("0.1", "1e-1");
        assert_same("15.5", "1.55e1");
        assert_same("150", "1.5e2");
        assert_differs("1", "1e05");
    }

    #[test]
    fn zero_absorbs_its_sign_and_its_scale() {
        for spelling in ["-0", "0.0", "-0.0", "0.000", "-0.000", "0e0", "0e100"] {
            assert_same("0", spelling);
        }
        assert_differs("0", "1");
        assert_differs("0", "-1");
    }

    #[test]
    fn sign_and_magnitude_matter() {
        assert_differs("1", "-1");
        assert_differs("1", "2");
        assert_differs("1", "10");
        assert_differs("1", "0.1");
        // Adjacent powers of ten.
        assert_differs("999", "1000");
        assert_differs("0.999", "1");
        assert_same("1000", "1e3");
    }

    #[test]
    fn precision_beyond_a_double_is_kept() {
        // These two differ in the twenty-fifth digit, which an f64 cannot represent.
        assert_differs("12345678901234567890123456", "12345678901234567890123457");
        assert_same(
            "12345678901234567890123456",
            "1.2345678901234567890123456e25",
        );
        assert_differs("0.1000000000000000000000001", "0.1000000000000000000000002");
        // A value an f64 would round to 1.0.
        assert_differs("1", "1.0000000000000000000000001");
    }

    #[test]
    fn long_trailing_zero_runs_collapse() {
        let long = format!("1.{}", "0".repeat(16_383));
        assert_same("1", &long);
        // One more fraction digit puts the display scale past PostgreSQL's 16383 ceiling.
        assert_refused(&format!("1.{}", "0".repeat(16_384)));
        // Shifting the exponent up by one brings it back into range.
        assert_accepted(&format!("1.{}e1", "0".repeat(16_384)));
    }

    #[test]
    fn display_scale_ceiling_is_16383() {
        assert_accepted("1e-16383");
        assert_refused("1e-16384");
        // Scale is fraction digits minus exponent, so a fraction digit costs one.
        assert_refused("1.5e-16383");
        assert_accepted("0.1e-16382");
        // The bound applies to a zero mantissa too.
        assert_accepted("0e-16383");
        assert_refused("0e-16384");
        assert_refused(&format!("0.{}", "0".repeat(16_384)));
    }

    #[test]
    fn integer_digit_ceiling_is_131072() {
        assert_accepted("1e131071");
        assert_refused("1e131072");
        assert_accepted("9.9e131071");
        assert_accepted(&"9".repeat(131_072));
        assert_refused(&"9".repeat(131_073));
        assert_accepted(&format!("{}.{}", "9".repeat(131_072), "9".repeat(16_383)));
        assert_refused(&format!("{}.{}", "9".repeat(131_072), "9".repeat(16_384)));
    }

    #[test]
    fn the_integer_ceiling_counts_significant_digits_not_written_ones() {
        // Leading zeros do not consume the budget; a literal reading misplaces this by three.
        assert_accepted("0.001e131074");
        assert_refused("0.001e131075");
        // Neither do trailing zeros.
        assert_accepted("10e131070");
        assert_refused("100e131070");
    }

    #[test]
    fn exponent_ceiling_is_half_of_i32_max() {
        // A zero mantissa isolates this bound from the other two. This file targets Pg18.
        assert_accepted("0e1073741823");
        assert_refused("0e1073741824");
        assert_accepted("0e131073");
        // An exponent too long for a machine integer is refused, not overflowed.
        assert_refused("1e99999999999999999999");
        assert_refused("-1e99999999999999999999");
        // Leading zeros in the exponent are legal JSON and do not inflate it.
        assert_accepted("1e0000000000000000005");
        assert_same("100000", "1e0000000000000000005");
    }

    #[test]
    fn the_exponent_ceiling_moved_between_postgresql_15_and_16() {
        // The one place the majors disagree, and only for zero at this exact exponent.
        let disputed = parse("0e1073741823");
        assert_eq!(
            encode::<Pg14>(&disputed),
            Err(CanonicalError::NumberOutOfRange)
        );
        assert_eq!(
            encode::<Pg15>(&disputed),
            Err(CanonicalError::NumberOutOfRange)
        );
        assert!(encode::<Pg16>(&disputed).is_ok());
        assert!(encode::<Pg17>(&disputed).is_ok());
        assert!(encode::<Pg18>(&disputed).is_ok());
        assert_eq!(
            equivalent::<Pg15>(&disputed, &disputed),
            Err(CanonicalError::NumberOutOfRange)
        );
        assert!(equivalent::<Pg16>(&disputed, &parse("0")).expect("accepted from 16 on"));

        // One step either side, every major agrees.
        let below = parse("0e1073741822");
        let above = parse("0e1073741824");
        assert!(encode::<Pg14>(&below).is_ok());
        assert!(encode::<Pg18>(&below).is_ok());
        assert!(encode::<Pg14>(&above).is_err());
        assert!(encode::<Pg18>(&above).is_err());

        // A non-zero mantissa is refused everywhere, needing over 131072 integer digits.
        let nonzero = parse("1e1073741823");
        assert!(encode::<Pg16>(&nonzero).is_err());
        assert!(encode::<Pg18>(&nonzero).is_err());
    }

    #[test]
    fn the_marker_changes_acceptance_and_nothing_else() {
        // A key written before a server upgrade stays valid after one.
        for text in [
            "0",
            "1",
            "1.00",
            "-1e-16383",
            r#"{"a":[1,{"b":"x"}]}"#,
            "0e1073741822",
        ] {
            let value = parse(text);
            let bytes = encode::<Pg14>(&value).expect("accepted on 14");
            assert_eq!(
                bytes,
                encode::<Pg15>(&value).expect("accepted on 15"),
                "{text}"
            );
            assert_eq!(
                bytes,
                encode::<Pg16>(&value).expect("accepted on 16"),
                "{text}"
            );
            assert_eq!(
                bytes,
                encode::<Pg17>(&value).expect("accepted on 17"),
                "{text}"
            );
            assert_eq!(
                bytes,
                encode::<Pg18>(&value).expect("accepted on 18"),
                "{text}"
            );
        }
    }
}

mod containers {
    use super::*;

    #[test]
    fn object_key_order_does_not_matter() {
        assert_same(r#"{"a":1,"b":2}"#, r#"{"b":2,"a":1}"#);
        assert_same(r#"{"aa":1,"b":2}"#, r#"{"b":2,"aa":1}"#);
        assert_same(r#"{"é":1,"e":2}"#, r#"{"e":2,"é":1}"#);
    }

    #[test]
    fn object_key_sets_must_match() {
        assert_differs(r#"{"a":1}"#, r#"{"b":1}"#);
        assert_differs(r#"{"a":1}"#, r#"{"a":1,"b":2}"#);
        assert_differs(r#"{"a":1}"#, "{}");
        // Same length, different bytes.
        assert_differs(r#"{"ab":1}"#, r#"{"ba":1}"#);
    }

    #[test]
    fn object_values_are_compared_recursively() {
        assert_same(r#"{"a":{"b":1.0}}"#, r#"{"a":{"b":1}}"#);
        assert_differs(r#"{"a":{"b":1}}"#, r#"{"a":{"b":2}}"#);
    }

    #[test]
    fn array_order_matters() {
        assert_same("[]", "[]");
        assert_same("[1.0,2.00]", "[1,2]");
        assert_differs("[1,2]", "[2,1]");
        assert_differs("[1]", "[1,1]");
        assert_differs("[]", "[null]");
        // Early and late mismatches.
        assert_differs("[9,2,3,4]", "[1,2,3,4]");
        assert_differs("[1,2,3,9]", "[1,2,3,4]");
    }

    #[test]
    fn arrays_and_objects_are_not_interchangeable() {
        assert_differs("[]", "{}");
        assert_differs("[1]", r#"{"0":1}"#);
        assert_differs("null", "[]");
        assert_differs("null", r#""null""#);
        assert_differs("1", r#""1""#);
        assert_differs("true", "1");
    }

    #[test]
    fn strings_compare_by_exact_bytes() {
        assert_same(r#""hello""#, r#""hello""#);
        assert_differs(r#""A""#, r#""a""#);
        // PostgreSQL applies no Unicode normalization: composed and decomposed differ.
        assert_differs(r#""é""#, r#""e\u0301""#);
        assert_same(r#""\ud83d\ude00""#, r#""😀""#);
    }

    #[test]
    fn large_arrays_stay_consistent() {
        let wide: Vec<Value> = (0..4096).map(|index| json!(index)).collect();
        let mut reversed = wide.clone();
        reversed.reverse();
        let (wide, reversed) = (Value::Array(wide), Value::Array(reversed));
        assert!(equivalent::<Pg18>(&wide, &wide).expect("in range"));
        assert!(!equivalent::<Pg18>(&wide, &reversed).expect("in range"));
        assert_eq!(
            encode::<Pg18>(&wide).expect("in range"),
            encode::<Pg18>(&wide).expect("in range")
        );
    }
}

mod limits {
    use super::*;

    #[test]
    fn nesting_is_capped_at_max_depth() {
        for build in [nested as fn(usize) -> Value, nested_objects] {
            assert!(encode::<Pg18>(&build(MAX_DEPTH)).is_ok());
            assert_eq!(
                encode::<Pg18>(&build(MAX_DEPTH + 1)),
                Err(CanonicalError::NestingLimit)
            );
            assert_eq!(
                equivalent::<Pg18>(&build(MAX_DEPTH + 1), &Value::Null),
                Err(CanonicalError::NestingLimit)
            );
            // The limit counts containers of either kind alike.
            assert!(equivalent::<Pg18>(&build(MAX_DEPTH), &build(MAX_DEPTH)).expect("at the cap"));
        }

        // Mixed containers count against one budget rather than two.
        let mut mixed = Value::Null;
        for level in 0..=MAX_DEPTH {
            mixed = if level % 2 == 0 {
                Value::Array(vec![mixed])
            } else {
                let mut object = serde_json::Map::new();
                object.insert("k".to_owned(), mixed);
                Value::Object(object)
            };
        }
        assert_eq!(encode::<Pg18>(&mixed), Err(CanonicalError::NestingLimit));
    }

    #[test]
    fn anything_serde_json_can_parse_is_within_the_cap() {
        // serde_json stops at 127, one below the cap, so a parsed value never trips it.
        let deepest = format!("{}1{}", "[".repeat(127), "]".repeat(127));
        let value: Value = serde_json::from_str(&deepest).expect("127 parses");
        assert!(encode::<Pg18>(&value).is_ok());
        assert!(
            serde_json::from_str::<Value>(&format!("{}1{}", "[".repeat(128), "]".repeat(128)))
                .is_err()
        );
    }
}

mod error_behaviour {
    use super::*;

    #[test]
    fn encode_into_restores_the_buffer_on_error() {
        let mut buffer = b"prefix".to_vec();
        encode_into::<Pg18>(&json!({"ok": 1}), &mut buffer).expect("in range");
        let after_success = buffer.clone();

        for refused in [parse("1e-16384"), nested(MAX_DEPTH + 1)] {
            assert!(encode_into::<Pg18>(&refused, &mut buffer).is_err());
            assert_eq!(buffer, after_success, "buffer must be restored exactly");
        }

        // The failure can also sit deep inside an otherwise fine document.
        let deep = parse(r#"{"a":[1,2,{"b":1e-16384}]}"#);
        assert!(encode_into::<Pg18>(&deep, &mut buffer).is_err());
        assert_eq!(buffer, after_success);
    }

    #[test]
    fn encode_into_appends_rather_than_replaces() {
        let mut buffer = b"prefix".to_vec();
        encode_into::<Pg18>(&json!(1), &mut buffer).expect("in range");
        assert!(buffer.starts_with(b"prefix"));
        assert_eq!(
            &buffer[6..],
            encode::<Pg18>(&json!(1)).expect("in range").as_slice()
        );
    }

    #[test]
    fn equivalent_reports_a_refused_number_behind_an_earlier_mismatch() {
        // The arrays differ at index 0, so a short-circuiting comparison would never look at
        // the refused number. Strictness requires the error anyway.
        let left = parse("[1,1e-16384]");
        let right = parse("[2,1e-16384]");
        assert_eq!(
            equivalent::<Pg18>(&left, &right),
            Err(CanonicalError::NumberOutOfRange)
        );

        // Identical values never need to interpret the number either.
        assert_eq!(
            equivalent::<Pg18>(&left, &left),
            Err(CanonicalError::NumberOutOfRange)
        );

        // A refusal on either side alone is enough.
        let refused = parse("[1e-16384]");
        let fine = parse("[1]");
        assert_eq!(
            equivalent::<Pg18>(&refused, &fine),
            Err(CanonicalError::NumberOutOfRange)
        );
        assert_eq!(
            equivalent::<Pg18>(&fine, &refused),
            Err(CanonicalError::NumberOutOfRange)
        );
    }

    #[test]
    fn equivalent_and_encode_accept_the_same_values() {
        let cases = [
            "1",
            "1e-16384",
            "1e-16383",
            "1e131072",
            "1e131071",
            "0e1073741824",
            r#"{"a":1e-16384}"#,
            "[[1e131072]]",
            r#"{"a":[1,{"b":1e-16384}]}"#,
            "[]",
            "{}",
            "null",
        ];
        for text in cases {
            let value = parse(text);
            assert_eq!(
                encode::<Pg18>(&value).is_ok(),
                equivalent::<Pg18>(&value, &value).is_ok(),
                "{text}: encode and equivalent disagree on acceptance"
            );
        }
    }

    #[test]
    fn a_value_breaking_two_rules_is_still_refused_by_both() {
        // The two walk objects in different orders, so they meet different violations first.
        // The variant is diagnostic; refusal is not, and must agree.
        let mut object = serde_json::Map::new();
        object.insert("aa".to_owned(), parse("1e-16384"));
        object.insert("b".to_owned(), nested(MAX_DEPTH + 1));
        let value = Value::Object(object);

        assert!(encode::<Pg18>(&value).is_err());
        assert!(equivalent::<Pg18>(&value, &value).is_err());
        assert_eq!(
            encode::<Pg18>(&value).is_ok(),
            equivalent::<Pg18>(&value, &value).is_ok(),
            "acceptance must agree even when several rules are broken"
        );

        // Mirrored, so the test does not depend on which walk wins.
        let mut mirrored = serde_json::Map::new();
        mirrored.insert("aa".to_owned(), nested(MAX_DEPTH + 1));
        mirrored.insert("b".to_owned(), parse("1e-16384"));
        let mirrored = Value::Object(mirrored);
        assert!(encode::<Pg18>(&mirrored).is_err());
        assert!(equivalent::<Pg18>(&mirrored, &mirrored).is_err());
    }
}
