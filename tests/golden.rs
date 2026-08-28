//! Byte-for-byte vectors pinning the encoding.
//!
//! Every expectation here is written out from the grammar in the README rather than
//! captured from the implementation. A change to any of these bytes is a format change and
//! must raise `ENCODING_VERSION`.

use postgres_jsonb_canonical::{encode, Pg18, ENCODING_VERSION, MAGIC};
use serde_json::Value;

/// Parses a spelling exactly. `json!` would route numbers through `f64` and lose the text.
fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("test input is valid JSON")
}

/// Expected bytes: the five-byte header followed by the node.
fn framed(node: &[u8]) -> Vec<u8> {
    let mut expected = MAGIC.to_vec();
    expected.push(ENCODING_VERSION);
    expected.extend_from_slice(node);
    expected
}

fn assert_golden(text: &str, node: &[u8]) {
    let actual = encode::<Pg18>(&parse(text)).expect("golden input must encode");
    assert_eq!(actual, framed(node), "encoding of {text} changed");
}

#[test]
fn header_is_pgjb_version_one() {
    assert_eq!(MAGIC, *b"PGJB");
    assert_eq!(ENCODING_VERSION, 1);
    assert_eq!(
        encode::<Pg18>(&Value::Null).expect("null encodes"),
        vec![0x50, 0x47, 0x4A, 0x42, 0x01, 0x00]
    );
}

#[test]
fn scalars() {
    assert_golden("null", &[0x00]);
    assert_golden("false", &[0x01]);
    assert_golden("true", &[0x02]);
}

#[test]
fn zero_has_one_encoding() {
    let zero = [0x03, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
    for spelling in [
        "0", "-0", "0.0", "-0.0", "0.000", "-0.000", "0e0", "0e100", "-0e-100",
    ] {
        assert_golden(spelling, &zero);
    }
}

#[test]
fn integers_carry_sign_exponent_count_and_digits() {
    // sign 1, exponent 0, one digit, ASCII '1'
    assert_golden("1", &[0x03, 0x01, 0, 0, 0, 0, 0, 0, 0, 1, b'1']);
    assert_golden("-1", &[0x03, 0x02, 0, 0, 0, 0, 0, 0, 0, 1, b'1']);
    // 100 normalizes to digit '1' at exponent 2
    assert_golden("100", &[0x03, 0x01, 0, 0, 0, 2, 0, 0, 0, 1, b'1']);
    assert_golden("1e2", &[0x03, 0x01, 0, 0, 0, 2, 0, 0, 0, 1, b'1']);
    // 12 keeps both digits at exponent 0
    assert_golden("12", &[0x03, 0x01, 0, 0, 0, 0, 0, 0, 0, 2, b'1', b'2']);
}

#[test]
fn negative_exponents_are_twos_complement() {
    // 0.1 is digit '1' at exponent -1
    assert_golden(
        "0.1",
        &[0x03, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 1, b'1'],
    );
    assert_golden(
        "1e-1",
        &[0x03, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 1, b'1'],
    );
    // 123.456 is digits "123456" at exponent -3, spanning the decimal point
    assert_golden(
        "123.456",
        &[
            0x03, 0x01, 0xFF, 0xFF, 0xFF, 0xFD, 0, 0, 0, 6, b'1', b'2', b'3', b'4', b'5', b'6',
        ],
    );
}

#[test]
fn trailing_zeros_never_reach_the_output() {
    let one = [0x03, 0x01, 0, 0, 0, 0, 0, 0, 0, 1, b'1'];
    for spelling in [
        "1", "1.0", "1.00", "1e0", "1E0", "1e+0", "1e-0", "0.1e1", "0.01e2",
    ] {
        assert_golden(spelling, &one);
    }
}

#[test]
fn strings_are_length_then_utf8() {
    assert_golden(r#""""#, &[0x04, 0, 0, 0, 0]);
    assert_golden(r#""hi""#, &[0x04, 0, 0, 0, 2, b'h', b'i']);
    // Four UTF-8 bytes, not one character.
    assert_golden(
        r#""\ud83d\ude00""#,
        &[0x04, 0, 0, 0, 4, 0xF0, 0x9F, 0x98, 0x80],
    );
    // PostgreSQL refuses an embedded NUL, so this is outside its domain, but the crate
    // still answers by bytes rather than inventing a refusal.
    assert_golden("\"a\\u0000b\"", &[0x04, 0, 0, 0, 3, b'a', 0x00, b'b']);
}

#[test]
fn arrays_are_count_then_elements_in_order() {
    assert_golden("[]", &[0x05, 0, 0, 0, 0]);
    assert_golden("[null]", &[0x05, 0, 0, 0, 1, 0x00]);
    assert_golden("[true,false]", &[0x05, 0, 0, 0, 2, 0x02, 0x01]);
    assert_golden("[[]]", &[0x05, 0, 0, 0, 1, 0x05, 0, 0, 0, 0]);
}

#[test]
fn object_keys_are_ordered_by_length_then_bytes() {
    assert_golden("{}", &[0x06, 0, 0, 0, 0]);

    // Source order is bb, a, cc, b, aaa. PostgreSQL orders a, b, bb, cc, aaa.
    let expected = [
        0x06, 0, 0, 0, 5, //
        0, 0, 0, 1, b'a', 0x00, //
        0, 0, 0, 1, b'b', 0x00, //
        0, 0, 0, 2, b'b', b'b', 0x00, //
        0, 0, 0, 2, b'c', b'c', 0x00, //
        0, 0, 0, 3, b'a', b'a', b'a', 0x00,
    ];
    assert_golden(
        r#"{"bb":null,"a":null,"cc":null,"b":null,"aaa":null}"#,
        &expected,
    );
}

#[test]
fn object_keys_carry_no_string_tag() {
    // A key is a bare length-prefixed run. Only a string value carries tag 0x04.
    assert_golden(
        r#"{"k":"k"}"#,
        &[0x06, 0, 0, 0, 1, 0, 0, 0, 1, b'k', 0x04, 0, 0, 0, 1, b'k'],
    );
}

#[test]
fn duplicate_source_keys_collapse_to_the_last_value() {
    // PostgreSQL keeps the last value, and so does serde_json's parser.
    let duplicated = encode::<Pg18>(&parse(r#"{"a":1,"a":2}"#)).expect("encodes");
    let last = encode::<Pg18>(&parse(r#"{"a":2}"#)).expect("encodes");
    assert_eq!(duplicated, last);
}

#[test]
fn nested_boundaries_are_pinned() {
    let expected = [
        0x06, 0, 0, 0, 1, // one pair
        0, 0, 0, 1, b'a', // key "a"
        0x05, 0, 0, 0, 2, // array of two
        0x03, 0x01, 0, 0, 0, 0, 0, 0, 0, 1, b'1', // number 1
        0x06, 0, 0, 0, 1, // nested object, one pair
        0, 0, 0, 1, b'b', // key "b"
        0x02, // true
    ];
    assert_golden(r#"{"a":[1.000,{"b":true}]}"#, &expected);
}

#[test]
fn distinct_types_never_share_bytes() {
    let spellings = [
        "null",
        "false",
        "true",
        "0",
        "1",
        r#""""#,
        r#""1""#,
        "[]",
        "[1]",
        "{}",
        r#"{"":null}"#,
    ];
    for (index, left) in spellings.iter().enumerate() {
        for right in &spellings[index + 1..] {
            assert_ne!(
                encode::<Pg18>(&parse(left)).expect("encodes"),
                encode::<Pg18>(&parse(right)).expect("encodes"),
                "{left} and {right} collide"
            );
        }
    }
}
