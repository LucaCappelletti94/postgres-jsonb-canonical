//! Structure-aware generator shared by every target.
//!
//! Feeding raw bytes to a JSON parser would spend the whole budget on syntax errors, so the
//! fuzzer builds values directly and only the number spellings are assembled from parts.

// Every target includes this whole module and uses a different part of it.
#![allow(dead_code)]

use arbitrary::{Arbitrary, Result, Unstructured};
use serde_json::{Map, Number, Value};

/// Exponents worth landing on, being the three ceilings PostgreSQL enforces and their
/// mirrors. The generator aims at these deliberately, because a uniformly random exponent
/// essentially never lands within a few steps of a boundary.
const INTERESTING: [i32; 9] = [
    0,
    // Rule 1, the written-exponent ceiling. Reachable only with a zero mantissa, since a
    // non-zero one trips rule 3 long before.
    1_073_741_823,
    -1_073_741_823,
    // Rule 2, the display-scale ceiling, which bites on negative exponents.
    -16_383,
    16_383,
    // Rule 3, the integer-digit ceiling. With one significant digit, `1e131071` sits
    // exactly on it and `1e131072` is one past.
    131_071,
    131_072,
    -131_071,
    -131_072,
];

/// A decimal exponent, drawn so that boundaries are reached often and in-range values
/// still dominate.
///
/// The distribution is hand-written rather than derived. A derived enum would split
/// evenly, which would put most numbers out of range and starve every target that builds
/// a whole document, since one refused number refuses the document.
#[derive(Debug, Clone, Copy)]
pub struct Exponent(pub i32);

impl<'a> Arbitrary<'a> for Exponent {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        Ok(Self(match u8::arbitrary(u)? {
            // Everyday magnitudes, always within every bound.
            0..=199 => i32::from(i8::arbitrary(u)?),
            // Within a few steps of a ceiling, which is where the arithmetic is most
            // likely to be wrong.
            200..=239 => {
                let which = usize::from(u8::arbitrary(u)?) % INTERESTING.len();
                INTERESTING[which].saturating_add(i32::from(i8::arbitrary(u)? % 3))
            }
            // Anywhere at all, including far outside anything PostgreSQL would take.
            _ => i32::arbitrary(u)?,
        }))
    }

    fn size_hint(depth: usize) -> (usize, Option<usize>) {
        arbitrary::size_hint::and(<u8 as Arbitrary>::size_hint(depth), (1, Some(4)))
    }
}

/// A number the crate is expected to accept, built from parts rather than filtered.
#[derive(Arbitrary, Debug, Clone)]
pub struct Spelling {
    pub negative: bool,
    /// Trimmed to at most 40 digits and never left empty.
    pub digits: Vec<u8>,
    /// How many of those digits fall after the decimal point.
    pub fraction: u8,
    pub exponent: Exponent,
    pub uppercase_marker: bool,
    pub explicit_plus: bool,
}

impl Spelling {
    /// Renders a JSON number. Never produces a leading zero on a multi-digit integer part,
    /// which JSON forbids.
    pub fn render(&self) -> String {
        let mut digits: String = self
            .digits
            .iter()
            .take(40)
            .map(|byte| char::from(b'0' + byte % 10))
            .collect();
        if digits.is_empty() {
            digits.push('0');
        }

        let fraction = usize::from(self.fraction) % (digits.len() + 1);
        let split = digits.len() - fraction;
        let (integer, fraction) = digits.split_at(split);
        let integer = integer.trim_start_matches('0');
        let integer = if integer.is_empty() { "0" } else { integer };

        let mantissa = if fraction.is_empty() {
            integer.to_owned()
        } else {
            format!("{integer}.{fraction}")
        };
        let marker = if self.uppercase_marker { 'E' } else { 'e' };
        let sign = if self.exponent.0 >= 0 && self.explicit_plus {
            "+"
        } else {
            ""
        };
        let lead = if self.negative { "-" } else { "" };
        format!("{lead}{mantissa}{marker}{sign}{}", self.exponent.0)
    }

    /// The same value written differently: pad the fraction, or move the point right and
    /// drop the exponent to match.
    pub fn respellings(&self) -> Vec<String> {
        respellings_of(&self.render())
    }
}

/// Every way this harness knows to rewrite a decimal spelling without changing its value.
///
/// Free-standing because the oracle target applies it to spellings that came from a real
/// server rather than from the generator, which is what lets a verified answer anchor a
/// mutation the server never saw.
pub fn respellings_of(rendered: &str) -> Vec<String> {
    let (mantissa, exponent) = match rendered.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => match exponent.parse::<i64>() {
            Ok(exponent) => (mantissa, exponent),
            Err(_) => return Vec::new(),
        },
        None => (rendered, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let sign = if integer.starts_with('-') { "-" } else { "" };
    let integer = integer.strip_prefix('-').unwrap_or(integer);

    let mut variants = vec![
        format!("{sign}{integer}.{fraction}0e{exponent}"),
        format!("{sign}{integer}.{fraction}000e{exponent}"),
    ];
    let all = format!("{integer}{fraction}");
    for shift in 1..=fraction.len() {
        let at = integer.len() + shift;
        let head = all[..at].trim_start_matches('0');
        let head = if head.is_empty() { "0" } else { head };
        let tail = &all[at..];
        let mantissa = if tail.is_empty() {
            head.to_owned()
        } else {
            format!("{head}.{tail}")
        };
        let Ok(shift) = i64::try_from(shift) else {
            continue;
        };
        variants.push(format!("{sign}{mantissa}e{}", exponent - shift));
    }
    variants
}

/// A JSON value the crate is expected to handle.
#[derive(Arbitrary, Debug, Clone)]
pub enum Shape {
    Null,
    Bool(bool),
    Number(Spelling),
    Text(String),
    Array(Vec<Shape>),
    Object(Vec<(String, Shape)>),
}

impl Shape {
    /// Builds the value. Depth is capped well inside the crate's limit so the targets
    /// exercise ordinary documents rather than the nesting error over and over.
    pub fn build(&self, budget: usize) -> Value {
        if budget == 0 {
            return Value::Null;
        }
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Number(spelling) => number(&spelling.render()),
            Self::Text(value) => Value::String(value.clone()),
            Self::Array(values) => {
                Value::Array(values.iter().map(|value| value.build(budget - 1)).collect())
            }
            Self::Object(pairs) => Value::Object(
                pairs
                    .iter()
                    .map(|(key, value)| (key.clone(), value.build(budget - 1)))
                    .collect::<Map<_, _>>(),
            ),
        }
    }

    /// The same value with every object's keys inserted in the opposite order.
    ///
    /// The generated pair list can carry the same key twice, and collecting it into a map
    /// keeps the last of them. Reversing the raw list would therefore keep a different
    /// value, which is a different document rather than a permutation of one. So the
    /// duplicates are resolved first, exactly as the forward build resolves them, and only
    /// the surviving pairs are reversed.
    pub fn build_reversed(&self, budget: usize) -> Value {
        if budget == 0 {
            return Value::Null;
        }
        match self {
            Self::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| value.build_reversed(budget - 1))
                    .collect(),
            ),
            Self::Object(pairs) => {
                let mut unique: Vec<(&str, &Self)> = Vec::new();
                for (key, value) in pairs {
                    match unique.iter_mut().find(|(seen, _)| *seen == key.as_str()) {
                        Some(slot) => slot.1 = value,
                        None => unique.push((key.as_str(), value)),
                    }
                }
                Value::Object(
                    unique
                        .into_iter()
                        .rev()
                        .map(|(key, value)| (key.to_owned(), value.build_reversed(budget - 1)))
                        .collect::<Map<_, _>>(),
                )
            }
            other => other.build(budget),
        }
    }
}

/// Parses a spelling into a `Value`, falling back to null if the generator slipped.
pub fn number(spelling: &str) -> Value {
    spelling
        .parse::<Number>()
        .map_or(Value::Null, Value::Number)
}
