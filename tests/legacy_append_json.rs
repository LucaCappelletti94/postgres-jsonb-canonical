//! Comparison against the encoder this crate replaces.
//!
//! subql builds group keys with `append_json` in `src/backend.rs`, reproduced below
//! verbatim from revision `be1561b`. Adopting this crate changes those bytes, so anyone
//! reviewing the swap needs to know two things: exactly how the format differs, and
//! whether the change is safe.
//!
//! Safety here means one thing only. A group key decides which rows collapse together, so
//! what must not change is the *partition*: which values are treated as equal. The bytes
//! are free to change, and they do. These tests pin both halves of that claim.

// This suite needs dev-dependencies, which are gated on little-endian so the big-endian
// job does not have to build them. See the comment in Cargo.toml.
#![cfg(target_endian = "little")]
// The reproduction is copied source. Keeping it identical to subql's is the point, so it
// is exempt from the lints that would ask us to improve it.
#![allow(clippy::pedantic, clippy::all)]

use std::collections::BTreeMap;

use postgres_jsonb_canonical::{encode, equivalent, Pg18};
use serde_json::Value;

// -- subql's encoder, reproduced ---------------------------------------------------------
//
// Verbatim from `subql` revision be1561b, `src/backend.rs`: `AppendPostcard`,
// `append_postcard`, `append_tagged` and `append_json`. Do not tidy these; a divergence
// here would make the comparison meaningless.

struct AppendPostcard<'a>(&'a mut Vec<u8>);

impl postcard::ser_flavors::Flavor for AppendPostcard<'_> {
    type Output = ();

    fn try_extend(&mut self, data: &[u8]) -> postcard::Result<()> {
        self.0.extend_from_slice(data);
        Ok(())
    }

    fn try_push(&mut self, data: u8) -> postcard::Result<()> {
        self.0.push(data);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Self::Output> {
        Ok(())
    }
}

fn append_postcard<T: serde::Serialize + ?Sized>(output: &mut Vec<u8>, value: &T) -> bool {
    postcard::serialize_with_flavor::<T, AppendPostcard<'_>, ()>(value, AppendPostcard(output))
        .is_ok()
}

fn append_tagged<T: serde::Serialize + ?Sized>(output: &mut Vec<u8>, tag: u8, value: &T) -> bool {
    output.push(tag);
    append_postcard(output, value)
}

fn append_json(value: &serde_json::Value, output: &mut Vec<u8>) -> bool {
    match value {
        serde_json::Value::Null => {
            output.push(0);
            true
        }
        serde_json::Value::Bool(value) => append_tagged(output, 1, value),
        serde_json::Value::Number(value) => {
            let Ok(number) = value.to_string().parse::<bigdecimal::BigDecimal>() else {
                return false;
            };
            append_tagged(output, 2, &number.normalized().to_string())
        }
        serde_json::Value::String(value) => append_tagged(output, 3, value),
        serde_json::Value::Array(values) => {
            let Ok(length) = u32::try_from(values.len()) else {
                return false;
            };
            output.push(4);
            output.extend_from_slice(&length.to_be_bytes());
            values.iter().all(|value| append_json(value, output))
        }
        serde_json::Value::Object(values) => {
            let Ok(length) = u32::try_from(values.len()) else {
                return false;
            };
            output.push(5);
            output.extend_from_slice(&length.to_be_bytes());
            let mut fields: Vec<_> = values.iter().collect();
            fields.sort_unstable_by(|left, right| {
                left.0
                    .len()
                    .cmp(&right.0.len())
                    .then_with(|| left.0.as_bytes().cmp(right.0.as_bytes()))
            });
            fields
                .into_iter()
                .all(|(name, value)| append_postcard(output, name) && append_json(value, output))
        }
    }
}

// -- harness -----------------------------------------------------------------------------

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("test input is valid JSON")
}

/// The legacy encoding, or `None` where the legacy encoder refused.
fn legacy(value: &Value) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    append_json(value, &mut output).then_some(output)
}

/// Values both encoders accept, chosen to cover every case where the two could disagree
/// about equality: number spellings, key order, key length ordering, nesting, and the
/// scalar types that must never collide.
fn shared_corpus() -> Vec<&'static str> {
    vec![
        "1",
        "1.0",
        "1.00",
        "1e0",
        "1E0",
        "0.01e2",
        "2",
        "-1",
        "10",
        "0.1",
        "0",
        "-0",
        "0.0",
        "-0.000",
        "0e100",
        "100",
        "1e2",
        "1.5e2",
        "150",
        "1.55e1",
        "15.5",
        "12345678901234567890123456",
        "1.2345678901234567890123456e25",
        "12345678901234567890123457",
        "null",
        "true",
        "false",
        "\"1\"",
        "\"\"",
        "\"A\"",
        "\"a\"",
        "\"é\"",
        "\"e\\u0301\"",
        "{}",
        r#"{"a":1}"#,
        r#"{"a":1.00}"#,
        r#"{"b":1}"#,
        r#"{"a":1,"b":2}"#,
        r#"{"b":2,"a":1}"#,
        r#"{"aa":1,"b":2}"#,
        r#"{"b":2,"aa":1}"#,
        r#"{"ab":1}"#,
        r#"{"ba":1}"#,
        "[]",
        "[1]",
        "[1.000]",
        "[1,1]",
        "[1,2]",
        "[2,1]",
        "[[1]]",
        "[[],[]]",
        r#"[{"a":[1,{"b":true}]}]"#,
        r#"[{"a":[1.0,{"b":true}]}]"#,
        r#"{"0":1}"#,
    ]
}

/// Groups the corpus by encoded bytes, so two encoders can be compared by the partition
/// they induce rather than by the bytes themselves.
fn partition(encoding: impl Fn(&Value) -> Option<Vec<u8>>) -> Vec<Vec<&'static str>> {
    let mut groups: BTreeMap<Vec<u8>, Vec<&'static str>> = BTreeMap::new();
    for text in shared_corpus() {
        let key = encoding(&parse(text)).expect("shared corpus encodes under both");
        groups.entry(key).or_default().push(text);
    }
    let mut groups: Vec<Vec<&'static str>> = groups.into_values().collect();
    groups.sort();
    groups
}

// -- the claims ---------------------------------------------------------------------------

#[test]
fn the_partition_is_unchanged() {
    // The property that makes the swap safe. Group identity is the partition, not the
    // bytes, and it must survive the change untouched.
    assert_eq!(
        partition(|value| encode::<Pg18>(value).ok()),
        partition(legacy),
        "the set of values that group together changed"
    );
}

#[test]
fn equivalent_agrees_with_the_legacy_encoder_on_every_pair() {
    // The same claim at the level of the predicate path rather than the group-key path,
    // since subql uses one for `Value::eq` and the other for group keys.
    let corpus = shared_corpus();
    for (index, left) in corpus.iter().enumerate() {
        for right in &corpus[index..] {
            let (left_value, right_value) = (parse(left), parse(right));
            let legacy_equal = legacy(&left_value) == legacy(&right_value);
            let ours = equivalent::<Pg18>(&left_value, &right_value).expect("corpus is in range");
            assert_eq!(ours, legacy_equal, "disagreed on `{left}` versus `{right}`");
        }
    }
}

#[test]
fn the_byte_format_changed_in_three_ways() {
    // Documented rather than merely observed, because a reviewer needs to know these are
    // deliberate. Each assertion names one difference.

    // 1. A standalone encoding now carries a magic and a version. The legacy component had
    //    neither, relying on subql's outer `SQGK` envelope for both.
    let ours = encode::<Pg18>(&parse("null")).expect("in range");
    let theirs = legacy(&parse("null")).expect("legacy encodes null");
    assert_eq!(ours, b"PGJB\x01\x00");
    assert_eq!(theirs, vec![0x00]);

    // 2. Lengths are fixed-width big-endian rather than postcard varints, and strings gain
    //    a tag. The legacy string component was tag 3 then a postcard varint length.
    let ours = encode::<Pg18>(&parse(r#""hi""#)).expect("in range");
    let theirs = legacy(&parse(r#""hi""#)).expect("legacy encodes strings");
    assert_eq!(ours, b"PGJB\x01\x04\x00\x00\x00\x02hi");
    assert_eq!(theirs, vec![0x03, 0x02, b'h', b'i']);

    // 3. Numbers are a sign, an exponent and the significant digits, not the decimal text
    //    that `BigDecimal::normalized().to_string()` happened to produce.
    let ours = encode::<Pg18>(&parse("100")).expect("in range");
    let theirs = legacy(&parse("100")).expect("legacy encodes numbers");
    assert_eq!(ours, b"PGJB\x01\x03\x01\x00\x00\x00\x02\x00\x00\x00\x011");
    assert_ne!(
        ours[5..],
        theirs[..],
        "the number payload is supposed to differ"
    );

    // Object key ordering is the one thing that did not change: length first, then bytes.
    let reordered = encode::<Pg18>(&parse(r#"{"bb":1,"a":2}"#)).expect("in range");
    let ordered = encode::<Pg18>(&parse(r#"{"a":2,"bb":1}"#)).expect("in range");
    assert_eq!(reordered, ordered);
    assert_eq!(
        legacy(&parse(r#"{"bb":1,"a":2}"#)),
        legacy(&parse(r#"{"a":2,"bb":1}"#))
    );
}

#[test]
fn the_new_encoder_refuses_strictly_more() {
    // Everything the new encoder accepts, the legacy one accepted too. The reverse does
    // not hold, and every case below is a value PostgreSQL itself rejects, so the legacy
    // encoder was minting group keys for data that could never have come out of a column.
    for text in [
        "1e-16384",     // display scale 16384, one past PostgreSQL's ceiling
        "1e131072",     // 131073 integer digits, one past PostgreSQL's ceiling
        "0e1073741824", // exponent one past PostgreSQL's ceiling
    ] {
        let value = parse(text);
        assert!(
            encode::<Pg18>(&value).is_err(),
            "{text} should be refused now"
        );
        assert!(legacy(&value).is_some(), "{text} was accepted before");
    }

    // Nothing in the shared corpus goes the other way.
    for text in shared_corpus() {
        let value = parse(text);
        if encode::<Pg18>(&value).is_ok() {
            assert!(
                legacy(&value).is_some(),
                "{text}: new accepts what legacy refused"
            );
        }
    }
}

#[test]
fn the_legacy_encoder_cannot_even_be_run_on_some_refused_input() {
    // `1e-2000000000` parses to a `BigDecimal` with a scale of two billion, and asking it
    // for `.normalized().to_string()` materialises a two-billion-character string. The
    // legacy encoder does that unconditionally, with no bound in front of it, so a single
    // crafted jsonb number exhausts memory. That path is deliberately not executed here.
    //
    // The new encoder decides from the text and never materialises digits it was not
    // given, so it simply refuses.
    let hostile = parse("1e-2000000000");
    assert!(encode::<Pg18>(&hostile).is_err());
}

#[test]
fn the_partition_has_real_structure() {
    // Guards `the_partition_is_unchanged` against going vacuous. Comparing two partitions
    // proves nothing if every value sits alone in its own group, so the corpus has to
    // actually make values collapse together, and the interesting collapses are named.
    let groups = partition(|value| encode::<Pg18>(value).ok());
    let collapsed = groups.iter().filter(|group| group.len() > 1).count();

    assert!(
        groups.len() < shared_corpus().len(),
        "no value collapsed with any other: {} groups for {} entries",
        groups.len(),
        shared_corpus().len()
    );
    assert!(
        collapsed >= 8,
        "only {collapsed} groups have more than one member"
    );

    let has = |members: &[&str]| {
        groups
            .iter()
            .any(|group| members.iter().all(|member| group.contains(member)))
    };
    assert!(
        has(&["1", "1.0", "1.00", "1e0", "1E0", "0.01e2"]),
        "number spellings"
    );
    assert!(
        has(&["0", "-0", "0.0", "-0.000", "0e100"]),
        "zero and its sign and scale"
    );
    assert!(has(&["100", "1e2"]), "exponent notation");
    assert!(
        has(&[r#"{"a":1,"b":2}"#, r#"{"b":2,"a":1}"#]),
        "object key order"
    );
    assert!(
        has(&[r#"{"aa":1,"b":2}"#, r#"{"b":2,"aa":1}"#]),
        "key length ordering"
    );
    assert!(has(&["[1]", "[1.000]"]), "numbers nested in an array");
}
