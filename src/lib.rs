#![no_std]
// Not in the manifest: `[lints]` reaches bench and test harness macros too.
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

extern crate alloc;

mod encode;
mod equal;
mod number;

use alloc::vec::Vec;
use serde_json::Value;

/// Version byte written after [`MAGIC`]. Any change to the produced bytes bumps this.
pub const ENCODING_VERSION: u8 = 1;

/// Fixed prefix of a standalone encoding, written before [`ENCODING_VERSION`].
pub const MAGIC: [u8; 4] = *b"PGJB";

/// Deepest nesting walked, in containers; `serde_json` parses at most 127.
pub const MAX_DEPTH: usize = 128;

/// Widest array, object or string, from PostgreSQL's `JB_CMASK`.
pub const MAX_CONTAINER_ELEMENTS: usize = 0x0FFF_FFFF;

mod sealed {
    pub trait Sealed {}
}

/// The PostgreSQL major a value must be valid for.
///
/// Majors differ only in which numbers they accept: `0e1073741823` is valid from 16 on and
/// an error before it. Encoded bytes are the same under every marker.
///
/// Sealed, because an implementation nobody checked against a server would defeat it.
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
/// Branch on `is_err`, not on the variant: [`encode`] and [`equivalent`] always agree on
/// refusal but walk objects in different orders, so a value breaking two rules can report
/// either one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CanonicalError {
    /// A number the named server's `numeric` input would reject.
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
/// Both sides are range-checked in full first, so an early mismatch cannot hide a refused
/// number.
///
/// ```
/// use postgres_jsonb_canonical::{equivalent, Pg17};
/// use serde_json::json;
///
/// let left = json!({"b": true, "a": 1.00});
/// let right = json!({"a": 1e0, "b": true});
/// assert!(equivalent::<Pg17>(&left, &right)?);
///
/// assert!(!equivalent::<Pg17>(&json!([1, 2]), &json!([2, 1]))?);
/// # Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
/// ```
pub fn equivalent<V: PgVersion>(left: &Value, right: &Value) -> Result<bool, CanonicalError> {
    equal::equivalent(left, right, V::MAX_EXPONENT)
}

/// Encodes `value` to bytes identical for, and only for, values PostgreSQL's `jsonb =`
/// considers equal.
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
/// The marker decides acceptance, never the bytes.
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

/// Appends the encoding of `value` to `output`, restoring its original length on error.
///
/// ```
/// use postgres_jsonb_canonical::{encode_into, Pg17};
/// use serde_json::{json, Value};
///
/// let mut key = b"row:".to_vec();
/// encode_into::<Pg17>(&json!([1]), &mut key)?;
/// let after_success = key.clone();
///
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
