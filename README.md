# postgres-jsonb-canonical

[![CI](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/postgres-jsonb-canonical.svg)](https://crates.io/crates/postgres-jsonb-canonical)
[![docs.rs](https://img.shields.io/docsrs/postgres-jsonb-canonical)](https://docs.rs/postgres-jsonb-canonical)
[![license](https://img.shields.io/crates/l/postgres-jsonb-canonical.svg)](https://github.com/LucaCappelletti94/postgres-jsonb-canonical/blob/main/LICENSE)
![msrv](https://img.shields.io/badge/msrv-1.81-blue.svg)
[![Codacy](https://app.codacy.com/project/badge/Grade/88ea19cf7fb443aa94035b9a57ceba46)](https://app.codacy.com/gh/LucaCappelletti94/postgres-jsonb-canonical/dashboard)

Canonical encoding and equality for PostgreSQL `jsonb`, without a connection.

PostgreSQL reads `1`, `1.0`, `1.00` and `1e0` as one number and object keys as a set, so comparing `serde_json::Value`s disagrees with the database. This crate encodes two values to identical bytes exactly when `jsonb =` calls them equal.

```rust
use postgres_jsonb_canonical::{encode, equivalent, Pg17};
use serde_json::json;

let left = json!({"b": true, "a": 1.00});
let right = json!({"a": 1e0, "b": true});

assert!(equivalent::<Pg17>(&left, &right)?);
assert_eq!(encode::<Pg17>(&left)?, encode::<Pg17>(&right)?);
# Ok::<_, postgres_jsonb_canonical::CanonicalError>(())
```

`encode` builds a standalone key. `encode_into` appends to one you are already building and restores it on error. `equivalent` compares without allocating.

## Pick a server major

Majors differ in which numbers they accept, never in the bytes: `0e1073741823` is zero from PostgreSQL 16 on and an error before it. `Pg14` through `Pg18` exist, each checked against a running server.

## What it refuses

Numbers PostgreSQL could not store: exponent above 1073741823 in magnitude, display scale above 16383, or more than 131072 integer digits. Nesting stops at 128 containers, one above what `serde_json` parses.

## One thing to know

This crate enables `serde_json`'s `arbitrary_precision`, and Cargo features unify across a build, so every user of `serde_json` in your binary gets it too. `Number` then keeps its original text and compares by spelling, which is why `equivalent` exists.

The library is `no_std` with `alloc`, forbids unsafe code, and depends only on `serde_json` and `thiserror`.
