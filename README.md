# postgres-jsonb-canonical

[![CI](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/postgres-jsonb-canonical.svg)](https://crates.io/crates/postgres-jsonb-canonical)
[![docs.rs](https://img.shields.io/docsrs/postgres-jsonb-canonical)](https://docs.rs/postgres-jsonb-canonical)
[![license](https://img.shields.io/crates/l/postgres-jsonb-canonical.svg)](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/blob/main/LICENSE)
![msrv](https://img.shields.io/badge/msrv-1.81-blue.svg)

Canonical encoding and equality for PostgreSQL `jsonb`, without a connection.

Two values encode to identical bytes exactly when PostgreSQL's `jsonb =` considers them equal. PostgreSQL reads `1`, `1.0`, `1.00` and `1e0` as one number and object keys as a set, so a group key or a predicate built on `serde_json::Value` comparison disagrees with the database.

```rust
use postgres_jsonb_canonical::{encode, equivalent, Pg17};
use serde_json::json;

let left = json!({"b": true, "a": 1.00});
let right = json!({"a": 1e0, "b": true});

assert!(equivalent::<Pg17>(&left, &right)?);
assert_eq!(encode::<Pg17>(&left)?, encode::<Pg17>(&right)?);
# Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
```

Every call names a server major, because majors differ in which numbers they accept and never in the bytes they produce: `0e1073741823` is zero from 16 on and an error before it. `Pg14` through `Pg18` exist, each checked against a running server.

A number is refused when its exponent exceeds 1073741823 in magnitude, its display scale exceeds 16383, or its integer digits exceed 131072. Scale belongs to the spelling rather than the value, so `1` followed by a point and 16384 zeros is refused although it means one. Nesting is capped at 128 containers, one above the 127 `serde_json` parses.

Depending on this crate turns on `serde_json`'s `arbitrary_precision` for every user of `serde_json` in the binary, because Cargo unifies features across a build. `serde_json::Number` then keeps its original text and compares by spelling, which is why `equivalent` exists.

`encode_into` appends to a key you are already building, restoring the buffer's length on error so a refused value leaves no valid-looking prefix. The crate is `no_std` with `alloc`, forbids unsafe code, and depends only on `serde_json` and `thiserror`. It covers `jsonb` and equality alone: PostgreSQL defines no equality operator for `json`, and btree ordering carries behaviour that equality does not need.
