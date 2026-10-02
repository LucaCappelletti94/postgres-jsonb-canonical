//! Canonical decimal form of a JSON number, derived from its text.
//!
//! Every decimal has exactly one form as a sign, significant digits without leading or
//! trailing zeros, and the power of ten of the last digit.

use crate::CanonicalError;

// The exponent ceiling moved between PostgreSQL 15 and 16, so the caller supplies it.

/// Largest display scale, PostgreSQL's `NUMERIC_DSCALE_MASK`.
const MAX_SCALE: i64 = 16_383;

/// Largest integer digit count, `(NUMERIC_WEIGHT_MAX + 1) * 4`.
const MAX_INTEGER_DIGITS: i64 = 131_072;

/// Sign of a canonical decimal; zero is its own case, since PostgreSQL drops its sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sign {
    Zero,
    Positive,
    Negative,
}

impl Sign {
    /// Tag byte written into the encoding.
    pub(crate) const fn byte(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::Positive => 1,
            Self::Negative => 2,
        }
    }
}

/// A JSON number in canonical form.
///
/// Digits are two borrowed slices because they straddle the decimal point in the source.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Decimal<'a> {
    pub(crate) sign: Sign,
    head: &'a [u8],
    tail: &'a [u8],
    /// Power of ten of the last significant digit. Zero when the value is zero.
    pub(crate) exponent: i32,
}

impl<'a> Decimal<'a> {
    /// The single canonical zero, absorbing negative zero.
    const ZERO: Self = Self {
        sign: Sign::Zero,
        head: &[],
        tail: &[],
        exponent: 0,
    };

    /// Number of significant digits.
    pub(crate) const fn digit_count(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    /// Significant digits in order, as ASCII bytes.
    pub(crate) fn digits(&self) -> impl Iterator<Item = u8> + 'a {
        self.head.iter().chain(self.tail.iter()).copied()
    }

    /// The two slices the digits are spread across, in order.
    pub(crate) const fn slices(&self) -> (&'a [u8], &'a [u8]) {
        (self.head, self.tail)
    }
}

impl PartialEq for Decimal<'_> {
    /// The slice split is an artefact of the source, so `12.3` equals `123e-1`.
    fn eq(&self, other: &Self) -> bool {
        self.sign == other.sign
            && self.exponent == other.exponent
            && self.digit_count() == other.digit_count()
            && self.digits().eq(other.digits())
    }
}

impl Eq for Decimal<'_> {}

/// Parses a JSON number into canonical form, refusing what the named server would refuse.
///
/// Total: a spelling that is not a JSON number is refused rather than panicked on.
pub(crate) fn parse(spelling: &str, max_exponent: i64) -> Result<Decimal<'_>, CanonicalError> {
    let bytes = spelling.as_bytes();
    let mut at = 0;

    let negative = bytes.first() == Some(&b'-');
    if negative {
        at += 1;
    }

    let integer = take_digits(bytes, &mut at);
    if integer.is_empty() {
        return Err(CanonicalError::NumberOutOfRange);
    }

    let fraction = if bytes.get(at) == Some(&b'.') {
        at += 1;
        let digits = take_digits(bytes, &mut at);
        if digits.is_empty() {
            return Err(CanonicalError::NumberOutOfRange);
        }
        digits
    } else {
        &bytes[at..at]
    };

    let exponent = match bytes.get(at) {
        Some(&byte) if byte | 0x20 == b'e' => {
            at += 1;
            take_exponent(bytes, &mut at, max_exponent)?
        }
        _ => 0,
    };

    if at != bytes.len() {
        return Err(CanonicalError::NumberOutOfRange);
    }

    // Rule 2: display scale, which applies whatever the mantissa is.
    let fraction_len =
        i64::try_from(fraction.len()).map_err(|_| CanonicalError::NumberOutOfRange)?;
    if fraction_len - exponent > MAX_SCALE {
        return Err(CanonicalError::NumberOutOfRange);
    }

    let Some((head, tail, trailing_zeros)) = split_significant(integer, fraction) else {
        // Rule 3 does not apply to zero: PostgreSQL accepts `0e131073`.
        return Ok(Decimal::ZERO);
    };

    let significant =
        i64::try_from(head.len() + tail.len()).map_err(|_| CanonicalError::NumberOutOfRange)?;
    let trailing = i64::try_from(trailing_zeros).map_err(|_| CanonicalError::NumberOutOfRange)?;
    let last_digit_exponent = exponent - fraction_len + trailing;

    // Rule 3: integer digits, from the normalized count. The raw count misplaces the
    // boundary for a spelling with leading zeros, such as `0.001e131074`.
    if significant + last_digit_exponent > MAX_INTEGER_DIGITS {
        return Err(CanonicalError::NumberOutOfRange);
    }

    // Rules 2 and 3 confine this to -16383..=131071.
    let exponent =
        i32::try_from(last_digit_exponent).map_err(|_| CanonicalError::NumberOutOfRange)?;

    Ok(Decimal {
        sign: if negative {
            Sign::Negative
        } else {
            Sign::Positive
        },
        head,
        tail,
        exponent,
    })
}

/// Consumes a run of ASCII digits starting at `at`.
fn take_digits<'a>(bytes: &'a [u8], at: &mut usize) -> &'a [u8] {
    let start = *at;
    // The count lands in one step: stepping the cursor per digit would hang under a `*=` mutant.
    let run = bytes[start..]
        .iter()
        .take_while(|&&b| b.is_ascii_digit())
        .count();
    *at += run;
    &bytes[start..*at]
}

/// Consumes an exponent; JSON permits leading zeros, so the digit run does not bound it.
fn take_exponent(bytes: &[u8], at: &mut usize, max_exponent: i64) -> Result<i64, CanonicalError> {
    let negative = match bytes.get(*at) {
        Some(b'+') => {
            *at += 1;
            false
        }
        Some(b'-') => {
            *at += 1;
            true
        }
        _ => false,
    };

    let digits = take_digits(bytes, at);
    if digits.is_empty() {
        return Err(CanonicalError::NumberOutOfRange);
    }

    let mut magnitude: i64 = 0;
    for &digit in digits.iter().skip_while(|&&digit| digit == b'0') {
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|value| value.checked_add(i64::from(digit - b'0')))
            .ok_or(CanonicalError::NumberOutOfRange)?;
        // Rule 1, applied as we go so a long digit run cannot run away.
        if magnitude > max_exponent {
            return Err(CanonicalError::NumberOutOfRange);
        }
    }

    Ok(if negative { -magnitude } else { magnitude })
}

/// Narrows to the significant run, returning its slices and the trailing zeros dropped,
/// or `None` when the value is zero.
fn split_significant<'a>(
    integer: &'a [u8],
    fraction: &'a [u8],
) -> Option<(&'a [u8], &'a [u8], usize)> {
    let total = integer.len() + fraction.len();

    let in_integer = integer.iter().take_while(|&&digit| digit == b'0').count();
    let leading = if in_integer == integer.len() {
        in_integer + fraction.iter().take_while(|&&digit| digit == b'0').count()
    } else {
        in_integer
    };
    if leading == total {
        return None;
    }

    let in_fraction = fraction
        .iter()
        .rev()
        .take_while(|&&digit| digit == b'0')
        .count();
    let trailing = if in_fraction == fraction.len() {
        in_fraction
            + integer
                .iter()
                .rev()
                .take_while(|&&digit| digit == b'0')
                .count()
    } else {
        in_fraction
    };

    // The window `leading..total - trailing` may straddle the two slices.
    let end = total - trailing;
    let head = &integer[leading.min(integer.len())..end.min(integer.len())];
    let tail = &fraction[leading.saturating_sub(integer.len())..end.saturating_sub(integer.len())];
    Some((head, tail, trailing))
}

#[cfg(test)]
mod tests {
    use super::{parse, Sign};
    use crate::CanonicalError;

    /// Accepted by every major, so shape is tested apart from bounds.
    const ANY: i64 = 1_073_741_822;

    /// Unreachable through `serde_json`'s public API, but must answer rather than panic.
    #[test]
    fn a_spelling_that_is_not_a_json_number_is_refused() {
        for spelling in [
            "",
            "-",
            "+",
            "+1",
            ".",
            ".1",
            "1.",
            "1..2",
            "e5",
            "1e",
            "1E",
            "1e+",
            "1e-",
            "abc",
            "1x",
            "nan",
            "NaN",
            "Infinity",
            "-Infinity",
            "1_000",
            " 1",
            "1 ",
            "1,5",
            "0x10",
        ] {
            assert_eq!(
                parse(spelling, ANY).err(),
                Some(CanonicalError::NumberOutOfRange),
                "{spelling:?} should be refused"
            );
        }
    }

    /// Accepting any byte as the marker would read `1x5` as `1e5`.
    #[test]
    fn only_e_introduces_an_exponent() {
        assert!(parse("1e5", ANY).is_ok());
        assert!(parse("1E5", ANY).is_ok());
        for spelling in ["1x5", "1d5", "1f5", "1F5", "1D5", "1.5x5", "1&5"] {
            assert_eq!(
                parse(spelling, ANY).err(),
                Some(CanonicalError::NumberOutOfRange),
                "{spelling:?} should not be read as an exponent"
            );
        }
    }

    /// The same spelling is accepted or refused depending on the server named.
    #[test]
    fn the_exponent_ceiling_comes_from_the_caller() {
        assert!(parse("0e1073741822", 1_073_741_822).is_ok());
        assert!(parse("0e1073741823", 1_073_741_822).is_err());
        assert!(parse("0e1073741823", 1_073_741_823).is_ok());
        assert!(parse("0e1073741824", 1_073_741_823).is_err());
    }

    /// The canonical triple, checked directly rather than through the bytes.
    #[test]
    fn normalization_produces_the_expected_triple() {
        let cases: [(&str, Sign, i32, &str); 8] = [
            ("0", Sign::Zero, 0, ""),
            ("-0.000", Sign::Zero, 0, ""),
            ("1", Sign::Positive, 0, "1"),
            ("-1", Sign::Negative, 0, "1"),
            ("1.00", Sign::Positive, 0, "1"),
            ("100", Sign::Positive, 2, "1"),
            ("123.456", Sign::Positive, -3, "123456"),
            ("0.00120", Sign::Positive, -4, "12"),
        ];
        for (spelling, sign, exponent, digits) in cases {
            let value = parse(spelling, ANY).expect("in range");
            assert_eq!(value.sign, sign, "{spelling}");
            assert_eq!(value.exponent, exponent, "{spelling}");
            let actual: alloc::vec::Vec<u8> = value.digits().collect();
            assert_eq!(actual, digits.as_bytes(), "{spelling}");
        }
    }

    /// One value produces equal triples however its digits straddle the point.
    #[test]
    fn equality_ignores_how_the_digits_are_split() {
        assert_eq!(parse("12.3", ANY).ok(), parse("123e-1", ANY).ok());
        assert_eq!(parse("1", ANY).ok(), parse("0.001e3", ANY).ok());
        assert_ne!(parse("1", ANY).ok(), parse("-1", ANY).ok());
        assert_ne!(parse("1", ANY).ok(), parse("10", ANY).ok());
    }
}
