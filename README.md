# postgres-jsonb-canonical

[![CI](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/postgres-jsonb-canonical.svg)](https://crates.io/crates/postgres-jsonb-canonical)
[![docs.rs](https://img.shields.io/docsrs/postgres-jsonb-canonical)](https://docs.rs/postgres-jsonb-canonical)
[![license](https://img.shields.io/crates/l/postgres-jsonb-canonical.svg)](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/blob/main/LICENSE)
![msrv](https://img.shields.io/badge/msrv-1.81-blue.svg)

Canonical encoding and equality for PostgreSQL `jsonb`.

Two values encode to identical bytes if, and only if, PostgreSQL's `jsonb =` considers them equal. PostgreSQL treats `1`, `1.0`, `1.00` and `1e0` as one number and object keys as a set, so a group key or predicate built on plain `serde_json::Value` comparison disagrees with the database. This crate closes that gap without a connection.

```rust
use postgres_jsonb_canonical::{encode, equivalent, Pg17};
use serde_json::json;

let left = json!({"b": true, "a": 1.00});
let right = json!({"a": 1e0, "b": true});

assert!(equivalent::<Pg17>(&left, &right)?);
assert_eq!(encode::<Pg17>(&left)?, encode::<Pg17>(&right)?);
# Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
```

`encode_into` appends to a key you are already building, restoring the buffer's original length on error so a refused value leaves no valid-looking prefix.

## What it refuses

PostgreSQL accepts a JSON number when its exponent is at most 1073741823 in magnitude, its display scale (fraction digits minus exponent) at most 16383, and its integer digits at most 131072. The crate refuses the rest rather than inventing an answer. Scale belongs to the spelling, not the value, so `1` followed by a point and 16384 zeros is refused though it means one.

`equivalent` checks both values in full before comparing, keeping it in agreement with `encode` about what is acceptable. Which reason they report is not part of that agreement, since the two walk objects in different orders: branch on whether there was an error, not on which one.

Nesting is capped at 128 containers, one above the 127 `serde_json` itself parses.

## Why the server version is part of the call

Between 15 and 16 the exponent ceiling moved up by one, so `0e1073741823` is a valid way of writing zero from 16 on and an error before it. Acceptance is all the marker changes, never the bytes, so a key written before a server upgrade stays valid after one. `Pg14` through `Pg18` exist, and the trait is sealed because each was checked against a running server.

```rust
use postgres_jsonb_canonical::{encode, Pg15, Pg16};
use serde_json::Value;

let zero: Value = serde_json::from_str("0e1073741823").unwrap();
assert!(encode::<Pg15>(&zero).is_err());
assert!(encode::<Pg16>(&zero).is_ok());
```

## Feature unification

This crate enables `serde_json`'s `arbitrary_precision`, and Cargo features are shared across a build, so depending on it turns that feature on for every other user of `serde_json` in the binary. `serde_json::Number` then holds the original text and its `PartialEq` becomes spelling equality, which is why `equivalent` exists.

## Encoding

An opaque equality identity, not PostgreSQL's on-disk format, and nothing decodes it. Integers are big-endian.

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

`digits` are ASCII without leading or trailing zeros and `exponent` is the power of ten of the last of them. Zero has the single encoding `0x00 0x00000000 0x00000000`, absorbing negative zero as PostgreSQL does. Object ordering copies PostgreSQL's own: shorter keys first, ties broken by bytes.

Golden tests pin every byte, and any change to them raises `ENCODING_VERSION`.

## Scope

`jsonb` only, and equality only. PostgreSQL defines no equality operator for `json`, MySQL and SQLite have their own JSON semantics, and btree ordering carries behaviour equality does not need. The library is `no_std` with `alloc`, forbids unsafe code, and depends only on `serde_json` and `thiserror`.
