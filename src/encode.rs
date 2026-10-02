//! Streaming canonical encoder; the grammar is in the README.

use alloc::vec::Vec;
use serde_json::Value;

use crate::{
    number::{self, Decimal},
    CanonicalError, ENCODING_VERSION, MAGIC, MAX_CONTAINER_ELEMENTS, MAX_DEPTH,
};

const TAG_NULL: u8 = 0x00;
const TAG_FALSE: u8 = 0x01;
const TAG_TRUE: u8 = 0x02;
const TAG_NUMBER: u8 = 0x03;
const TAG_STRING: u8 = 0x04;
const TAG_ARRAY: u8 = 0x05;
const TAG_OBJECT: u8 = 0x06;

/// Writes magic, version and value; the caller restores `output` on error.
pub(crate) fn encode_into(
    value: &Value,
    output: &mut Vec<u8>,
    max_exponent: i64,
) -> Result<(), CanonicalError> {
    output.extend_from_slice(&MAGIC);
    output.push(ENCODING_VERSION);
    let mut scratch = Vec::new();
    node(value, output, &mut scratch, MAX_DEPTH, max_exponent)
}

/// Encodes one value, where `depth` is the remaining container budget.
///
/// One `scratch` serves every object: each sorts a window at its end and truncates back,
/// so cost does not scale with the number of objects.
fn node<'a>(
    value: &'a Value,
    output: &mut Vec<u8>,
    scratch: &mut Vec<(&'a str, &'a Value)>,
    depth: usize,
    max_exponent: i64,
) -> Result<(), CanonicalError> {
    match value {
        Value::Null => output.push(TAG_NULL),
        Value::Bool(false) => output.push(TAG_FALSE),
        Value::Bool(true) => output.push(TAG_TRUE),
        Value::Number(value) => {
            output.push(TAG_NUMBER);
            number(&number::parse(value.as_str(), max_exponent)?, output)?;
        }
        Value::String(value) => {
            output.push(TAG_STRING);
            string(value, output)?;
        }
        Value::Array(values) => {
            let Some(depth) = depth.checked_sub(1) else {
                return Err(CanonicalError::NestingLimit);
            };
            output.push(TAG_ARRAY);
            output.extend_from_slice(&count(values.len())?.to_be_bytes());
            for value in values {
                node(value, output, scratch, depth, max_exponent)?;
            }
        }
        Value::Object(values) => {
            let Some(depth) = depth.checked_sub(1) else {
                return Err(CanonicalError::NestingLimit);
            };
            output.push(TAG_OBJECT);
            output.extend_from_slice(&count(values.len())?.to_be_bytes());

            let base = scratch.len();
            scratch.extend(values.iter().map(|(key, value)| (key.as_str(), value)));
            // PostgreSQL's `lengthCompareJsonbString`: byte length first, then bytes.
            scratch[base..].sort_unstable_by_key(|(key, _)| (key.len(), key.as_bytes()));

            let end = scratch.len();
            for index in base..end {
                // Copied out so the child may grow `scratch`; it truncates back to `end`.
                let (key, value) = scratch[index];
                let outcome = string(key, output)
                    .and_then(|()| node(value, output, scratch, depth, max_exponent));
                if let Err(error) = outcome {
                    scratch.truncate(base);
                    return Err(error);
                }
            }
            scratch.truncate(base);
        }
    }
    Ok(())
}

/// Writes a length-prefixed UTF-8 run, for strings and object keys alike.
fn string(value: &str, output: &mut Vec<u8>) -> Result<(), CanonicalError> {
    output.extend_from_slice(&count(value.len())?.to_be_bytes());
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

/// Writes sign, exponent of the last digit, digit count, digits.
fn number(value: &Decimal<'_>, output: &mut Vec<u8>) -> Result<(), CanonicalError> {
    output.push(value.sign.byte());
    output.extend_from_slice(&value.exponent.to_be_bytes());
    output.extend_from_slice(&count(value.digit_count())?.to_be_bytes());
    let (head, tail) = value.slices();
    output.extend_from_slice(head);
    output.extend_from_slice(tail);
    Ok(())
}

/// Narrows a length to the wire width, refusing what PostgreSQL could not store.
fn count(value: usize) -> Result<u32, CanonicalError> {
    if value > MAX_CONTAINER_ELEMENTS {
        return Err(CanonicalError::ContainerTooLarge);
    }
    u32::try_from(value).map_err(|_| CanonicalError::ContainerTooLarge)
}

#[cfg(test)]
mod tests {
    use super::{count, MAX_CONTAINER_ELEMENTS};
    use crate::CanonicalError;

    /// Unreachable publicly without allocating 268435456 elements.
    #[test]
    fn the_width_ceiling_is_inclusive() {
        assert_eq!(count(0), Ok(0));
        assert_eq!(count(1), Ok(1));
        assert_eq!(count(MAX_CONTAINER_ELEMENTS), Ok(0x0FFF_FFFF));
        assert_eq!(
            count(MAX_CONTAINER_ELEMENTS + 1),
            Err(CanonicalError::ContainerTooLarge)
        );
        assert_eq!(count(usize::MAX), Err(CanonicalError::ContainerTooLarge));
    }
}
