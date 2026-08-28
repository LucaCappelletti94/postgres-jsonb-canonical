# postgres-jsonb-canonical

Canonical encoding and equality for PostgreSQL `jsonb`.

Two values encode to identical bytes if, and only if, PostgreSQL's `jsonb =` considers them equal. That is the whole contract. PostgreSQL treats `1`, `1.0`, `1.00` and `1e0` as the same number and treats object keys as a set, so a group key or an equality predicate built on plain `serde_json::Value` comparison disagrees with the database. This crate closes that gap without a database connection.

```rust
use postgres_jsonb_canonical::{encode, equivalent, Pg17};
use serde_json::json;

let left = json!({"b": true, "a": 1.00});
let right = json!({"a": 1e0, "b": true});

assert!(equivalent::<Pg17>(&left, &right)?);
assert_eq!(encode::<Pg17>(&left)?, encode::<Pg17>(&right)?);
# Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
```

The `Pg17` names the server the values have to be valid for. It costs nothing at runtime and does not change the bytes, but it does decide which numbers are refused, because the servers do not all agree. See below.

Use `encode_into` to append a value to a key you are already building. On error it truncates back to the length it had on entry, so a refused value never leaves a valid-looking prefix behind.

```rust
use postgres_jsonb_canonical::{encode_into, Pg17};
use serde_json::{json, Value};

let mut key = b"row:".to_vec();
encode_into::<Pg17>(&json!({"id": 7}), &mut key)?;
let good = key.clone();

let refused: Value = serde_json::from_str("1e-16384").unwrap();
assert!(encode_into::<Pg17>(&refused, &mut key).is_err());
assert_eq!(key, good);
# Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
```

## What it refuses

`1e-16384` above is a number PostgreSQL will not store, and the crate refuses it rather than inventing an answer. PostgreSQL accepts a JSON number when its exponent is at most 1073741823 in magnitude, its display scale (fraction digits minus exponent) is at most 16383, and its integer digit count is at most 131072. All three bounds were measured against PostgreSQL 14, 17 and 18, which agree exactly.

Scale is a property of how a number is written rather than of its value, so a perfectly ordinary value can be spelled out of range: `1` followed by a decimal point and 16384 zeros is refused even though it means one.

`equivalent` checks both values in full before comparing, so it refuses such a number wherever it sits, even when an earlier difference already settled the answer. That keeps `equivalent` and `encode` in agreement about which values are acceptable.

Nesting is capped at 128 containers. `serde_json` itself parses at most 127, so any value it produced always encodes, and only a value assembled in memory can hit the cap.

Whether a value is refused is part of the contract, and both functions always agree on it. Which reason they give is not: a value that breaks two rules at once gets whichever the walk met first, and the two do not walk in the same order. Branch on whether there was an error, not on which one.

## Why the server version is part of the call

Servers do not all accept the same numbers. Somewhere between 15 and 16 the ceiling on a written exponent moved up by one, so `0e1073741823` is a perfectly good way of writing zero on 16 and later and an error on 14 and 15. That is the whole difference, and it only ever shows up for zero written with that one exponent, but a crate whose job is to agree with the server cannot quietly pick a side.

```rust
use postgres_jsonb_canonical::{encode, Pg15, Pg16};
use serde_json::Value;

let zero: Value = serde_json::from_str("0e1073741823").unwrap();
assert!(encode::<Pg15>(&zero).is_err());
assert!(encode::<Pg16>(&zero).is_ok());
```

Acceptance is the only thing the marker changes. Two values that both encode produce identical bytes whichever server is named, so a key written before an upgrade is still valid after one. `Pg14` through `Pg18` are available, and the trait behind them is sealed because every one was checked against a running server of that major.

## Feature unification

This crate turns on `serde_json`'s `arbitrary_precision` feature, and Cargo features are shared across a build. Depending on this crate therefore switches `arbitrary_precision` on for every other user of `serde_json` in the same binary. That changes `serde_json::Number` into a string holding the original text, so `Number`'s own `PartialEq` becomes spelling equality and `1.0` stops equalling `1.00` at the `serde_json` layer. The crate cannot avoid this, and it is why the `equivalent` function exists.

## Encoding

The bytes are an opaque equality identity, not PostgreSQL's on-disk format, and nothing decodes them. Integers are big-endian.

```text
standalone := MAGIC VERSION node
MAGIC      := "PGJB"
VERSION    := 0x01

node := 0x00                                 null
      | 0x01                                 false
      | 0x02                                 true
      | 0x03 number
      | 0x04 u32be(byte_len) utf8_bytes      string
      | 0x05 u32be(count) node*              array, source order
      | 0x06 u32be(count) pair*              object, pairs sorted by (key_byte_len, key_bytes)

pair   := u32be(key_byte_len) key_utf8_bytes node

number := sign i32be(exponent) u32be(digit_count) digits
sign   := 0x00 zero | 0x01 positive | 0x02 negative
```

`digits` are ASCII with no leading and no trailing zero, and `exponent` is the power of ten of the last of them. Zero is the single encoding `0x00 0x00000000 0x00000000`, which also absorbs negative zero because PostgreSQL drops that sign on input. Object ordering copies PostgreSQL's own on-disk key order, shorter keys first and ties broken by bytes.

Every byte is pinned by golden tests. Any change to them raises `ENCODING_VERSION`.

## Scope

`jsonb` only. PostgreSQL defines no equality operator for the `json` type, MySQL and SQLite have their own JSON semantics, and none of those belong here. Ordering is also out of scope: PostgreSQL's btree order for `jsonb` carries behaviour that equality does not need, including an empty array that sorts below null.

The library is `no_std` with `alloc`, forbids unsafe code, and depends only on `serde_json` and `thiserror`.
