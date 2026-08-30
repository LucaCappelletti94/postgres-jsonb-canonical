# CHANGELOG

All notable changes to postgres-jsonb-canonical are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to pre-1.0 semantic versioning (breaking changes are allowed in minor releases until 1.0).

Any change to the bytes `encode` produces raises `ENCODING_VERSION` and is called out under its own heading, because it invalidates every key already stored.

## Unreleased

Becomes 0.1.0 on first release.

### Added

- `equivalent`, `encode` and `encode_into`. Two values encode to identical bytes if, and only if, PostgreSQL's `jsonb =` considers them equal. `encode_into` restores the buffer's original length on error, so a refused value leaves no valid-looking prefix in a larger key.
- `PgVersion` and the markers `Pg14` through `Pg18`, naming the server a value must be valid for. Majors differ only in which numbers they accept, never in the bytes: `0e1073741823` is valid from 16 on and an error before it. The trait is sealed, so adding a major is not a breaking change.
- `CanonicalError`, refusing a number outside PostgreSQL's numeric ceilings, a container wider than `JB_CMASK`, or nesting past `MAX_DEPTH`. Refusal is contractual and `encode` and `equivalent` always agree on it; which variant is reported is diagnostic only, since the two walk objects in different orders.
- `MAGIC`, `ENCODING_VERSION`, `MAX_DEPTH` and `MAX_CONTAINER_ELEMENTS`.
- `oracle/postgres-jsonb.tsv`, recorded from live servers, replayed by the test suite without one.

### Encoding

- `ENCODING_VERSION` 1. Big-endian integers, objects ordered as PostgreSQL orders them on disk, numbers as a sign, the exponent of the last significant digit, and the digits themselves. The grammar is in the README and every byte is pinned by golden tests.
