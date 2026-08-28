#![no_std]
// Scoped here rather than in the manifest: `[lints]` reaches every target, and the
// benchmark and test harness macros generate items no one can document.
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

extern crate alloc;

mod encode;
mod equal;
mod number;

use alloc::vec::Vec;
use serde_json::Value;

pub use number::MAX_DIGITS;

/// Version byte written after [`MAGIC`]. Any change to the produced bytes bumps this.
pub const ENCODING_VERSION: u8 = 1;

/// Fixed prefix of a standalone encoding, written before [`ENCODING_VERSION`].
pub const MAGIC: [u8; 4] = *b"PGJB";

/// Deepest nesting this crate walks, counted in containers.
///
/// `serde_json` parses at most 127 levels, so a value it produced always encodes. Only a
/// [`Value`] assembled in memory can exceed this.
pub const MAX_DEPTH: usize = 128;

/// Widest array or object this crate encodes.
///
/// Copied from PostgreSQL's `JB_CMASK`, so the crate refuses exactly the widths PostgreSQL
/// itself cannot store. String byte lengths share the bound, which is the same 28-bit field.
pub const MAX_CONTAINER_ELEMENTS: usize = 0x0FFF_FFFF;

mod sealed {
    pub trait Sealed {}
}

/// The PostgreSQL major a value must be valid for.
///
/// Servers do not all accept the same numbers. Between 15 and 16 the ceiling on a written
/// exponent moved up by one, so `0e1073741823` is a legal jsonb value on 16 and later and
/// an error on 14 and 15. Naming the server at the call site keeps that difference visible
/// instead of guessing.
///
/// Acceptance is the only thing that varies. Two values that both encode produce the same
/// bytes on every version, so a key written against one server stays valid against another.
///
/// The trait is sealed. Every implementation here was checked against a running server of
/// that major, and an implementation that was not would defeat the point.
pub trait PgVersion: sealed::Sealed {
    /// Largest written exponent this server accepts, in either direction.
    const MAX_EXPONENT: i64;
}

/// PostgreSQL 14.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pg14;

/// PostgreSQL 15.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pg15;

/// PostgreSQL 16.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pg16;

/// PostgreSQL 17.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pg17;

/// PostgreSQL 18.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pg18;

macro_rules! version {
    ($marker:ty, $max_exponent:expr) => {
        impl sealed::Sealed for $marker {}
        impl PgVersion for $marker {
            const MAX_EXPONENT: i64 = $max_exponent;
        }
    };
}

version!(Pg14, 1_073_741_822);
version!(Pg15, 1_073_741_822);
version!(Pg16, 1_073_741_823);
version!(Pg17, 1_073_741_823);
version!(Pg18, 1_073_741_823);

/// Reason a [`Value`] has no canonical encoding.
///
/// Whether a value is refused is contractual and [`encode`] and [`equivalent`] always
/// agree on it. *Which* variant comes back is diagnostic only. A value that breaks more
/// than one rule at once reports whichever the walk reached first, and the two functions
/// do not walk in the same order: the encoder visits object keys sorted, because that is
/// what produces canonical bytes, while the comparison visits them in whatever order the
/// map yields, because sorting would cost it an allocation. Do not branch on the variant
/// to decide whether a value is usable. Branch on `is_err`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CanonicalError {
    /// A number the named server's `numeric` input would reject.
    ///
    /// Also covers a spelling that is not a JSON number at all, which PostgreSQL rejects
    /// too. `serde_json`'s public API cannot produce one.
    #[error("jsonb number is outside the PostgreSQL numeric domain")]
    NumberOutOfRange,
    /// A string, array or object wider than [`MAX_CONTAINER_ELEMENTS`].
    #[error("jsonb container exceeds the canonical format limit")]
    ContainerTooLarge,
    /// Nesting deeper than [`MAX_DEPTH`].
    #[error("jsonb nesting exceeds MAX_DEPTH")]
    NestingLimit,
}

/// Reports whether PostgreSQL's `jsonb =` would consider the two values equal.
///
/// Both values are range-checked in full before comparison, so a mismatch early in the
/// traversal never hides a number the server would refuse.
///
/// ```
/// use postgres_jsonb_canonical::{equivalent, Pg17};
/// use serde_json::json;
///
/// // Spelling, key order and trailing zeros do not matter.
/// let left = json!({"b": true, "a": 1.00});
/// let right = json!({"a": 1e0, "b": true});
/// assert!(equivalent::<Pg17>(&left, &right)?);
///
/// // Array order does.
/// assert!(!equivalent::<Pg17>(&json!([1, 2]), &json!([2, 1]))?);
/// # Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
/// ```
pub fn equivalent<V: PgVersion>(left: &Value, right: &Value) -> Result<bool, CanonicalError> {
    equal::equivalent(left, right, V::MAX_EXPONENT)
}

/// Encodes `value` to bytes that are identical for, and only for, values PostgreSQL's
/// `jsonb =` considers equal.
///
/// ```
/// use postgres_jsonb_canonical::{encode, Pg17, MAGIC};
/// use serde_json::json;
///
/// let left = encode::<Pg17>(&json!({"b": 1, "a": [1.0, "x"]}))?;
/// let right = encode::<Pg17>(&json!({"a": [1.000, "x"], "b": 1e0}))?;
/// assert_eq!(left, right);
/// assert!(left.starts_with(&MAGIC));
/// # Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
/// ```
///
/// The bytes do not depend on the server named, only on whether it accepts the value.
///
/// ```
/// use postgres_jsonb_canonical::{encode, Pg14, Pg18};
/// use serde_json::json;
///
/// assert_eq!(encode::<Pg14>(&json!(1.00))?, encode::<Pg18>(&json!(1.00))?);
/// # Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
/// ```
pub fn encode<V: PgVersion>(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut output = Vec::new();
    encode_into::<V>(value, &mut output)?;
    Ok(output)
}

/// Appends the encoding of `value` to `output`.
///
/// On error `output` is truncated back to the length it had on entry, so a refused value
/// never leaves a valid-looking prefix inside a larger key.
///
/// ```
/// use postgres_jsonb_canonical::{encode_into, Pg17};
/// use serde_json::{json, Value};
///
/// let mut key = b"row:".to_vec();
/// encode_into::<Pg17>(&json!([1]), &mut key)?;
/// let after_success = key.clone();
///
/// // A number PostgreSQL cannot store leaves the buffer untouched.
/// let refused: Value = serde_json::from_str("[1e-16384]").unwrap();
/// assert!(encode_into::<Pg17>(&refused, &mut key).is_err());
/// assert_eq!(key, after_success);
/// # Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
/// ```
pub fn encode_into<V: PgVersion>(
    value: &Value,
    output: &mut Vec<u8>,
) -> Result<(), CanonicalError> {
    let restore_to = output.len();
    match encode::encode_into(value, output, V::MAX_EXPONENT) {
        Ok(()) => Ok(()),
        Err(error) => {
            output.truncate(restore_to);
            Err(error)
        }
    }
}
