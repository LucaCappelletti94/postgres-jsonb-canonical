//! Structure-aware generator shared by every target.
//!
//! Feeding raw bytes to a JSON parser would spend the whole budget on syntax errors, so the
//! fuzzer builds values directly and only the number spellings are assembled from parts.

// Every target includes this whole module and uses a different part of it.
#![allow(dead_code)]

use arbitrary::Arbitrary;
use serde_json::{Map, Number, Value};

/// A number the crate is expected to accept, built from parts rather than filtered.
#[derive(Arbitrary, Debug, Clone)]
pub struct Spelling {
    pub negative: bool,
    /// Trimmed to at most 40 digits and never left empty.
    pub digits: Vec<u8>,
    /// How many of those digits fall after the decimal point.
    pub fraction: u8,
    pub exponent: i16,
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
        let sign = if self.exponent >= 0 && self.explicit_plus {
            "+"
        } else {
            ""
        };
        let lead = if self.negative { "-" } else { "" };
        format!("{lead}{mantissa}{marker}{sign}{}", self.exponent)
    }

    /// The same value written differently: pad the fraction, or move the point right and
    /// drop the exponent to match.
    pub fn respellings(&self) -> Vec<String> {
        let rendered = self.render();
        let Some((mantissa, exponent)) = rendered.split_once(['e', 'E']) else {
            return Vec::new();
        };
        let Ok(exponent) = exponent.parse::<i64>() else {
            return Vec::new();
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
