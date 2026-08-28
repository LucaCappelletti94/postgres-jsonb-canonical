//! Direct canonical equality, without building either encoding.

use serde_json::Value;

use crate::{number, CanonicalError, MAX_CONTAINER_ELEMENTS, MAX_DEPTH};

/// Reports whether PostgreSQL's `jsonb =` would consider the two values equal.
///
/// Both sides are validated in full before anything is compared. Without that, a mismatch
/// near the start would let a number PostgreSQL cannot store go unnoticed, and `equivalent`
/// would then accept a pair that [`crate::encode`] refuses.
pub(crate) fn equivalent(
    left: &Value,
    right: &Value,
    max_exponent: i64,
) -> Result<bool, CanonicalError> {
    validate(left, MAX_DEPTH, max_exponent)?;
    validate(right, MAX_DEPTH, max_exponent)?;
    compare(left, right, MAX_DEPTH, max_exponent)
}

/// Checks depth, container widths and number ranges without comparing or normalizing.
///
/// This is the cheap half: it counts digits and never performs arithmetic on them, so it
/// costs far less than the comparison it precedes. It allocates nothing.
fn validate(value: &Value, depth: usize, max_exponent: i64) -> Result<(), CanonicalError> {
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::Number(value) => number::parse(value.as_str(), max_exponent).map(|_| ()),
        Value::String(value) => width(value.len()),
        Value::Array(values) => {
            let Some(depth) = depth.checked_sub(1) else {
                return Err(CanonicalError::NestingLimit);
            };
            width(values.len())?;
            values
                .iter()
                .try_for_each(|value| validate(value, depth, max_exponent))
        }
        Value::Object(values) => {
            let Some(depth) = depth.checked_sub(1) else {
                return Err(CanonicalError::NestingLimit);
            };
            width(values.len())?;
            values.iter().try_for_each(|(key, value)| {
                width(key.len())?;
                validate(value, depth, max_exponent)
            })
        }
    }
}

/// Compares two validated values, stopping at the first difference.
///
/// The error paths are unreachable once [`validate`] has accepted both sides. They are
/// propagated rather than asserted away so that no branch can answer wrongly if that
/// invariant is ever broken.
fn compare(
    left: &Value,
    right: &Value,
    depth: usize,
    max_exponent: i64,
) -> Result<bool, CanonicalError> {
    match (left, right) {
        (Value::Null, Value::Null) => Ok(true),
        (Value::Bool(left), Value::Bool(right)) => Ok(left == right),
        (Value::Number(left), Value::Number(right)) => {
            let (left, right) = (left.as_str(), right.as_str());
            // Identical spellings are identical values, so the common case never parses.
            if left == right {
                return Ok(true);
            }
            Ok(number::parse(left, max_exponent)? == number::parse(right, max_exponent)?)
        }
        (Value::String(left), Value::String(right)) => Ok(left == right),
        (Value::Array(left), Value::Array(right)) => {
            let Some(depth) = depth.checked_sub(1) else {
                return Err(CanonicalError::NestingLimit);
            };
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right) {
                if !compare(left, right, depth, max_exponent)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Value::Object(left), Value::Object(right)) => {
            let Some(depth) = depth.checked_sub(1) else {
                return Err(CanonicalError::NestingLimit);
            };
            // Equal sizes plus every left key found in right means the key sets match, so
            // neither side needs sorting and nothing is allocated.
            if left.len() != right.len() {
                return Ok(false);
            }
            for (key, left) in left {
                let Some(right) = right.get(key) else {
                    return Ok(false);
                };
                if !compare(left, right, depth, max_exponent)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Refuses a container or string wider than PostgreSQL's 28-bit length field.
const fn width(value: usize) -> Result<(), CanonicalError> {
    if value > MAX_CONTAINER_ELEMENTS {
        return Err(CanonicalError::ContainerTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{width, MAX_CONTAINER_ELEMENTS};
    use crate::CanonicalError;

    /// Same ceiling as the encoder's, and it has to agree with it or a value could compare
    /// equal to something it cannot encode.
    #[test]
    fn the_width_ceiling_is_inclusive() {
        assert_eq!(width(0), Ok(()));
        assert_eq!(width(MAX_CONTAINER_ELEMENTS), Ok(()));
        assert_eq!(
            width(MAX_CONTAINER_ELEMENTS + 1),
            Err(CanonicalError::ContainerTooLarge)
        );
        assert_eq!(width(usize::MAX), Err(CanonicalError::ContainerTooLarge));
    }
}
