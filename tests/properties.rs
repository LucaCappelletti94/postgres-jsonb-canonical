//! Property tests over generated values.
//!
//! Generators produce spellings inside PostgreSQL's accepted domain by construction rather
//! than filtering, so no case is silently discarded. `bigdecimal` serves as an independent
//! oracle for number normalization.

use core::str::FromStr;

use bigdecimal::BigDecimal;
use postgres_jsonb_canonical::{encode, encode_into, equivalent, CanonicalError, Pg18};
use proptest::prelude::*;
use serde_json::{Map, Value};

/// A number spelling PostgreSQL accepts, together with spellings of the same value.
///
/// Exponents stay small so the display scale and integer digit bounds cannot be reached by
/// accident. `tests/contract.rs` covers the bounds themselves.
fn in_range_number() -> impl Strategy<Value = String> {
    (any::<bool>(), "[0-9]{1,30}", 0usize..8, -20i32..20).prop_map(
        |(negative, digits, fraction, exponent)| {
            let fraction = fraction.min(digits.len());
            let split = digits.len() - fraction;
            let mantissa = if fraction == 0 {
                digits
            } else {
                format!("{}.{}", &digits[..split], &digits[split..])
            };
            // JSON forbids a leading zero on a multi-digit integer part.
            let mantissa = trim_leading_zeros(&mantissa);
            format!("{}{mantissa}e{exponent}", if negative { "-" } else { "" })
        },
    )
}

/// Removes JSON-illegal leading zeros from the integer part of a mantissa.
fn trim_leading_zeros(mantissa: &str) -> String {
    let (integer, rest) = match mantissa.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (mantissa, None),
    };
    let trimmed = integer.trim_start_matches('0');
    let integer = if trimmed.is_empty() { "0" } else { trimmed };
    rest.map_or_else(
        || integer.to_owned(),
        |fraction| format!("{integer}.{fraction}"),
    )
}

/// Every spelling of the same value this generator knows how to write.
///
/// Three transformations, each value-preserving: pad the fraction with zeros, move the
/// decimal point right while lowering the exponent by the same amount, and change the case
/// of the exponent marker.
fn respelled(spelling: &str) -> Vec<String> {
    let (mantissa, exponent) =
        spelling
            .split_once(['e', 'E'])
            .map_or((spelling, 0i64), |(mantissa, exponent)| {
                (
                    mantissa,
                    exponent.parse().expect("generated exponent parses"),
                )
            });
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let sign = if integer.starts_with('-') { "-" } else { "" };
    let integer = integer.strip_prefix('-').unwrap_or(integer);

    let mut variants = vec![spelling.to_ascii_uppercase()];

    for extra in 1..=3usize {
        let padding = "0".repeat(extra);
        variants.push(format!("{sign}{integer}.{fraction}{padding}e{exponent}"));
    }

    // Moving the point right by `shift` digits multiplies by ten that many times, so the
    // exponent drops by the same amount.
    let digits = format!("{integer}{fraction}");
    for shift in 0..=fraction.len() {
        let at = integer.len() + shift;
        let head = digits[..at].trim_start_matches('0');
        let head = if head.is_empty() { "0" } else { head };
        let tail = &digits[at..];
        let mantissa = if tail.is_empty() {
            head.to_owned()
        } else {
            format!("{head}.{tail}")
        };
        let exponent = exponent - i64::try_from(shift).expect("shift is small");
        variants.push(format!("{sign}{mantissa}e{exponent}"));
    }

    variants
}

/// A value tree of bounded size and depth.
fn any_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        in_range_number().prop_map(|spelling| parse(&spelling)),
        "[a-zA-Z0-9 é😀]{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(5, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec(("[a-z]{1,3}|[a-z]{5,7}", inner), 0..6)
                .prop_map(|pairs| { Value::Object(pairs.into_iter().collect::<Map<_, _>>()) }),
        ]
    })
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("generated JSON is valid")
}

/// Rebuilds a value with every object's keys visited in a rotated order.
fn permute_keys(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(permute_keys).collect()),
        Value::Object(values) => {
            let mut pairs: Vec<_> = values
                .iter()
                .map(|(key, value)| (key.clone(), permute_keys(value)))
                .collect();
            pairs.reverse();
            Value::Object(pairs.into_iter().collect())
        }
        other => other.clone(),
    }
}

proptest! {
    #[test]
    fn equivalence_is_reflexive(value in any_value()) {
        prop_assert!(equivalent::<Pg18>(&value, &value)?);
    }

    #[test]
    fn equivalence_is_symmetric(left in any_value(), right in any_value()) {
        prop_assert_eq!(equivalent::<Pg18>(&left, &right)?, equivalent::<Pg18>(&right, &left)?);
    }

    #[test]
    fn equivalence_is_transitive(a in any_value(), b in any_value(), c in any_value()) {
        if equivalent::<Pg18>(&a, &b)? && equivalent::<Pg18>(&b, &c)? {
            prop_assert!(equivalent::<Pg18>(&a, &c)?);
        }
    }

    #[test]
    fn equivalence_matches_byte_identity(left in any_value(), right in any_value()) {
        prop_assert_eq!(equivalent::<Pg18>(&left, &right)?, encode::<Pg18>(&left)? == encode::<Pg18>(&right)?);
    }

    #[test]
    fn encoding_is_deterministic(value in any_value()) {
        prop_assert_eq!(encode::<Pg18>(&value)?, encode::<Pg18>(&value)?);
    }

    #[test]
    fn object_key_order_never_reaches_the_bytes(value in any_value()) {
        let permuted = permute_keys(&value);
        prop_assert_eq!(encode::<Pg18>(&value)?, encode::<Pg18>(&permuted)?);
        prop_assert!(equivalent::<Pg18>(&value, &permuted)?);
    }

    #[test]
    fn array_order_is_observable(values in prop::collection::vec(any_value(), 2..6)) {
        let forward = Value::Array(values.clone());
        let mut backward = values;
        backward.reverse();
        let backward = Value::Array(backward);
        // Reversal changes the bytes unless the sequence is a palindrome of equal values.
        let palindrome = equivalent::<Pg18>(&forward, &backward)?;
        prop_assert_eq!(palindrome, encode::<Pg18>(&forward)? == encode::<Pg18>(&backward)?);
    }

    #[test]
    fn respelling_a_number_never_changes_it(spelling in in_range_number()) {
        let original = parse(&spelling);
        for variant in respelled(&spelling) {
            let variant = parse(&variant);
            prop_assert!(
                equivalent::<Pg18>(&original, &variant)?,
                "{spelling} should equal its respelling {variant}"
            );
            prop_assert_eq!(encode::<Pg18>(&original)?, encode::<Pg18>(&variant)?);
        }
    }

    #[test]
    fn normalization_agrees_with_bigdecimal(left in in_range_number(), right in in_range_number()) {
        let oracle = BigDecimal::from_str(&left).expect("oracle parses")
            == BigDecimal::from_str(&right).expect("oracle parses");
        prop_assert_eq!(equivalent::<Pg18>(&parse(&left), &parse(&right))?, oracle);
    }

    #[test]
    fn canonicalizing_is_idempotent(spelling in in_range_number()) {
        // The digits and exponent the encoder emits are themselves a valid spelling of the
        // same value, and re-encoding that spelling reproduces the bytes.
        let value = parse(&spelling);
        let bytes = encode::<Pg18>(&value)?;
        let round_trip = parse(&canonical_spelling(&bytes));
        prop_assert_eq!(encode::<Pg18>(&round_trip)?, bytes);
    }

    #[test]
    fn encode_into_keeps_a_prefix_on_success(prefix in prop::collection::vec(any::<u8>(), 0..16), value in any_value()) {
        let mut buffer = prefix.clone();
        encode_into::<Pg18>(&value, &mut buffer)?;
        prop_assert_eq!(&buffer[..prefix.len()], &prefix[..]);
        let standalone = encode::<Pg18>(&value)?;
        prop_assert_eq!(&buffer[prefix.len()..], standalone.as_slice());
    }

    #[test]
    fn encode_into_restores_a_prefix_on_failure(prefix in prop::collection::vec(any::<u8>(), 0..16), value in any_value()) {
        // Planting a refused number anywhere inside must leave the prefix untouched.
        let poisoned = Value::Array(vec![value, parse("1e-16384")]);
        let mut buffer = prefix.clone();
        prop_assert_eq!(encode_into::<Pg18>(&poisoned, &mut buffer), Err(CanonicalError::NumberOutOfRange));
        prop_assert_eq!(buffer, prefix);
    }

    #[test]
    fn a_mismatch_at_any_position_is_found(
        values in prop::collection::vec(any_value(), 1..8),
        at in 0usize..8,
    ) {
        let at = at % values.len();
        let mut changed = values.clone();
        // Replace one element with something no generated value can equal.
        changed[at] = Value::String("\u{0}sentinel".to_owned());
        let original = Value::Array(values);
        let changed = Value::Array(changed);
        prop_assert!(!equivalent::<Pg18>(&original, &changed)?);
        prop_assert_ne!(encode::<Pg18>(&original)?, encode::<Pg18>(&changed)?);
    }
}

/// Reads the digits and exponent back out of a single-number encoding and rewrites them as
/// a JSON number. Only used to prove idempotence.
fn canonical_spelling(bytes: &[u8]) -> String {
    // header(5) tag(1) sign(1) exponent(4) digit_count(4)
    let sign = bytes[6];
    let exponent = i32::from_be_bytes(bytes[7..11].try_into().expect("four bytes"));
    let digits = core::str::from_utf8(&bytes[15..]).expect("digits are ASCII");
    match sign {
        0 => "0".to_owned(),
        1 => format!("{digits}e{exponent}"),
        _ => format!("-{digits}e{exponent}"),
    }
}
