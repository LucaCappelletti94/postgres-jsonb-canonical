//! Differential tests against real PostgreSQL servers.
//!
//! PostgreSQL is the specification, so these are the tests that can actually falsify the
//! crate. Every supported major runs the same families against a server of that major,
//! using the matching [`PgVersion`] marker: which number spellings the server accepts, how
//! `jsonb =` partitions a corpus, how `GROUP BY` partitions it, and how duplicate keys
//! collapse.
//!
//! Each family asks the server once for the whole corpus rather than once per case.

// `#[derive(QueryableByName)]` expands to `Self { spelling: spelling, .. }`. Clippy blames
// the field spans rather than the macro, and an allow on the struct does not reach the
// expansion, so the exemption has to sit here.
#![allow(clippy::redundant_field_names)]

use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};

use diesel::{
    prelude::*,
    sql_types::{Array, Bool, Text},
};
use postgres_jsonb_canonical::{encode, equivalent, Pg14, Pg15, Pg16, Pg17, Pg18, PgVersion};
use serde_json::Value;
use testcontainers::{core::IntoContainerPort, runners::SyncRunner, ImageExt};
use testcontainers_modules::postgres::Postgres;

diesel::table! {
    /// Corpus rows the server partitions for us.
    jsonb_cases (id) {
        /// Index into the corpus list, so a server answer maps back to a spelling.
        id -> Integer,
        /// The value as PostgreSQL stores it.
        body -> Jsonb,
    }
}

/// Table and the one helper the query DSL cannot express, installed before the first
/// connection. Migration-style DDL.
const INIT_SQL: &str = "
CREATE TABLE jsonb_cases (id INTEGER PRIMARY KEY, body JSONB NOT NULL);

-- The DSL has no way to attempt a cast and recover from the error, which is exactly what
-- an acceptance probe is.
CREATE FUNCTION jsonb_accepts(spelling TEXT) RETURNS BOOLEAN LANGUAGE plpgsql AS $$
BEGIN
  PERFORM spelling::jsonb;
  RETURN TRUE;
EXCEPTION WHEN others THEN
  RETURN FALSE;
END $$;
";

#[derive(Debug, thiserror::Error)]
enum HarnessError {
    #[error("starting the postgres container")]
    Container(#[from] testcontainers::TestcontainersError),
    #[error("connecting to postgres")]
    Connect(#[from] diesel::ConnectionError),
    #[error("running a query")]
    Query(#[from] diesel::result::Error),
}

/// One spelling and whether PostgreSQL accepted it.
#[derive(QueryableByName)]
struct Acceptance {
    #[diesel(sql_type = Text)]
    spelling: String,
    #[diesel(sql_type = Bool)]
    accepted: bool,
}

/// Normalized text of a jsonb value PostgreSQL parsed from raw source.
#[derive(QueryableByName)]
struct Normalized {
    #[diesel(sql_type = Text)]
    normalized: String,
}

/// Number spellings whose acceptance the crate claims to reproduce.
///
/// Owned strings because the interesting cases are hundreds of thousands of digits long.
fn acceptance_corpus() -> Vec<String> {
    let mut corpus: Vec<String> = [
        // Ordinary values.
        "0",
        "-0",
        "1",
        "-1",
        "1.0",
        "1.00",
        "1e0",
        "1E0",
        "1e+0",
        "1e-0",
        "0.1",
        "1e-1",
        "100",
        "1e2",
        "1.5e2",
        "1.55e1",
        "12345678901234567890123456",
        // Exponent bound, isolated by a zero mantissa. 1073741823 is the one value the
        // supported majors disagree about, so it is the reason the marker exists.
        "0e1073741822",
        "0e1073741823",
        "0e1073741824",
        "0.0e1073741823",
        "0e131073",
        "0e1000000",
        "1e1073741823",
        "1e99999999999999999999",
        "-1e99999999999999999999",
        "1e0000000000000000005",
        // Display scale bound.
        "1e-16383",
        "1e-16384",
        "1.5e-16383",
        "0.1e-16382",
        "0e-16383",
        "0e-16384",
        // Integer digit bound.
        "1e131071",
        "1e131072",
        "9.9e131071",
        "10e131070",
        "100e131070",
        "0.001e131074",
        "0.001e131075",
    ]
    .iter()
    .map(|spelling| (*spelling).to_owned())
    .collect();

    // Long spellings, kept out of the literal list so it stays readable.
    corpus.push("9".repeat(131_072));
    corpus.push("9".repeat(131_073));
    corpus.push(format!("1.{}", "0".repeat(16_383)));
    corpus.push(format!("1.{}", "0".repeat(16_384)));
    corpus.push(format!("1.{}e1", "0".repeat(16_384)));
    corpus.push(format!("0.{}", "0".repeat(16_384)));
    corpus.push(format!("{}.{}", "9".repeat(131_072), "9".repeat(16_383)));
    corpus.push(format!("{}.{}", "9".repeat(131_072), "9".repeat(16_384)));
    corpus
}

/// Values whose equality and grouping the crate claims to reproduce.
///
/// Every entry must be one every supported major accepts, since they are inserted into a
/// jsonb column on all of them.
fn equality_corpus() -> Vec<&'static str> {
    vec![
        // Numbers that must collapse together.
        "1",
        "1.0",
        "1.00",
        "1e0",
        "1E0",
        "0.01e2",
        // ... and ones that must not.
        "2",
        "-1",
        "10",
        "0.1",
        // Zero in all its spellings.
        "0",
        "-0",
        "0.0",
        "-0.000",
        "0e100",
        // Beyond what a double can hold.
        "12345678901234567890123456",
        "1.2345678901234567890123456e25",
        "12345678901234567890123457",
        // Scalars of other types that must never collide with the numbers above.
        "null",
        "true",
        "false",
        "\"1\"",
        "\"\"",
        "\"A\"",
        "\"a\"",
        "\"é\"",
        "\"e\\u0301\"",
        "\"😀\"",
        // Objects: key order, key length ordering, key sets.
        "{}",
        r#"{"a":1}"#,
        r#"{"a":1.00}"#,
        r#"{"b":1}"#,
        r#"{"a":1,"b":2}"#,
        r#"{"b":2,"a":1}"#,
        r#"{"aa":1,"b":2}"#,
        r#"{"b":2,"aa":1}"#,
        r#"{"ab":1}"#,
        r#"{"ba":1}"#,
        r#"{"é":1,"e":2}"#,
        r#"{"e":2,"é":1}"#,
        // Arrays: order, length, nesting.
        "[]",
        "[1]",
        "[1.000]",
        "[1,1]",
        "[1,2]",
        "[2,1]",
        "[[1]]",
        "[[],[]]",
        r#"[{"a":[1,{"b":true}]}]"#,
        r#"[{"a":[1.0,{"b":true}]}]"#,
        // A container that must not equal a scalar of the same shape.
        r#"{"0":1}"#,
    ]
}

/// Object spellings PostgreSQL parses but `serde_json::Value` cannot hold, so they can only
/// reach the server as raw text.
const DUPLICATE_KEY_SOURCES: [&str; 4] = [
    r#"{"a":1,"a":2}"#,
    r#"{"a":1,"b":2,"a":3}"#,
    r#"{"k":{"d":1,"d":2}}"#,
    r#"{"a":1,"a":1.00}"#,
];

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("corpus entry is valid JSON")
}

/// Serializes container creation.
///
/// Five servers starting at once overruns the Docker daemon's request deadline. The lock is
/// held only while a container is being created and becoming ready, so the query phases
/// still overlap.
static STARTUP: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn startup_lock() -> MutexGuard<'static, ()> {
    STARTUP.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts one server and runs every family against it.
fn run_major<V: PgVersion>(major: &str) -> Result<(), HarnessError> {
    let (container, url) = {
        let _serialized = startup_lock();
        let container = Postgres::default()
            .with_init_sql(INIT_SQL.as_bytes().to_vec())
            .with_tag(major)
            .start()?;
        let url = format!(
            "postgres://postgres:postgres@{}:{}/postgres",
            container.get_host()?,
            container.get_host_port_ipv4(5432.tcp())?
        );
        (container, url)
    };
    let connection = &mut PgConnection::establish(&url)?;

    check_server_major(connection, major)?;
    check_acceptance::<V>(connection, major)?;
    let corpus = load_corpus(connection)?;
    check_pairwise_equality::<V>(connection, major, &corpus)?;
    check_grouping::<V>(connection, major, &corpus)?;
    check_duplicate_keys::<V>(connection, major)?;

    drop(container);
    Ok(())
}

/// Guards against a mislabelled image silently testing the wrong server.
fn check_server_major(connection: &mut PgConnection, major: &str) -> Result<(), HarnessError> {
    let reported: Normalized =
        diesel::sql_query("SELECT split_part(version(), ' ', 2) AS normalized")
            .get_result(connection)?;
    assert!(
        reported.normalized == major || reported.normalized.starts_with(&format!("{major}.")),
        "asked for postgres {major} but got {}",
        reported.normalized
    );
    Ok(())
}

/// The crate refuses exactly the number spellings this server refuses.
fn check_acceptance<V: PgVersion>(
    connection: &mut PgConnection,
    major: &str,
) -> Result<(), HarnessError> {
    let corpus = acceptance_corpus();

    // One round trip for the whole corpus. `unnest` with a recovering cast is beyond the
    // query DSL, so this is the justified raw statement.
    let answers: Vec<Acceptance> = diesel::sql_query(
        "SELECT spelling, jsonb_accepts(spelling) AS accepted \
         FROM unnest($1::text[]) AS spelling",
    )
    .bind::<Array<Text>, _>(&corpus)
    .load(connection)?;

    assert_eq!(answers.len(), corpus.len(), "pg {major}: lost rows");
    for answer in answers {
        let ours = encode::<V>(&parse(&answer.spelling)).is_ok();
        assert_eq!(
            ours,
            answer.accepted,
            "pg {major}: disagreed on {}",
            elide(&answer.spelling)
        );
    }
    Ok(())
}

/// `equivalent` agrees with `jsonb =` on every pair of the corpus.
fn check_pairwise_equality<V: PgVersion>(
    connection: &mut PgConnection,
    major: &str,
    corpus: &BTreeMap<i32, &'static str>,
) -> Result<(), HarnessError> {
    let (left, right) = diesel::alias!(jsonb_cases as left_side, jsonb_cases as right_side);
    let answers: Vec<(i32, i32, bool)> = left
        .inner_join(right.on(left.field(jsonb_cases::id).ge(right.field(jsonb_cases::id))))
        .select((
            left.field(jsonb_cases::id),
            right.field(jsonb_cases::id),
            left.field(jsonb_cases::body)
                .eq(right.field(jsonb_cases::body)),
        ))
        .load(connection)?;

    let expected_pairs = corpus.len() * (corpus.len() + 1) / 2;
    assert_eq!(
        answers.len(),
        expected_pairs,
        "pg {major}: wrong pair count"
    );

    for (left_id, right_id, server) in answers {
        let left_text = corpus[&left_id];
        let right_text = corpus[&right_id];
        let (left_value, right_value) = (parse(left_text), parse(right_text));

        let ours = equivalent::<V>(&left_value, &right_value).expect("corpus is in range");
        assert_eq!(ours, server, "pg {major}: `{left_text}` = `{right_text}`");

        let bytes_match = encode::<V>(&left_value).expect("in range")
            == encode::<V>(&right_value).expect("in range");
        assert_eq!(
            bytes_match, server,
            "pg {major}: bytes disagree with the server on `{left_text}` = `{right_text}`"
        );
    }
    Ok(())
}

/// PostgreSQL's `GROUP BY` partition is the canonical-byte partition.
///
/// `GROUP BY` reaches its answer by hashing or sorting rather than by calling `=`, so this
/// is a genuinely separate check from the pairwise family.
fn check_grouping<V: PgVersion>(
    connection: &mut PgConnection,
    major: &str,
    corpus: &BTreeMap<i32, &'static str>,
) -> Result<(), HarnessError> {
    // Representative and size of each server-side group.
    let mut server: Vec<(i32, i64)> = jsonb_cases::table
        .group_by(jsonb_cases::body)
        .select((
            diesel::dsl::min(jsonb_cases::id).assume_not_null(),
            diesel::dsl::count_star(),
        ))
        .load(connection)?;
    server.sort_unstable();

    // The same partition, from canonical bytes alone.
    let mut groups: BTreeMap<Vec<u8>, (i32, i64)> = BTreeMap::new();
    for (id, text) in corpus {
        let key = encode::<V>(&parse(text)).expect("corpus is in range");
        let entry = groups.entry(key).or_insert((*id, 0));
        entry.0 = entry.0.min(*id);
        entry.1 += 1;
    }
    let mut ours: Vec<(i32, i64)> = groups.into_values().collect();
    ours.sort_unstable();

    assert_eq!(ours, server, "pg {major}: grouping differs");
    Ok(())
}

/// Duplicate keys: PostgreSQL keeps the last value, and so must the crate.
///
/// The source text cannot be held by `serde_json::Value`, so it reaches the server as text.
fn check_duplicate_keys<V: PgVersion>(
    connection: &mut PgConnection,
    major: &str,
) -> Result<(), HarnessError> {
    for source in DUPLICATE_KEY_SOURCES {
        // Casting a text literal to jsonb and back is not a table query, so there is no
        // typed DSL form of it.
        let answer: Normalized = diesel::sql_query("SELECT ($1::text)::jsonb::text AS normalized")
            .bind::<Text, _>(source)
            .get_result(connection)?;

        let server_side = parse(&answer.normalized);
        let ours = parse(source);
        assert!(
            equivalent::<V>(&ours, &server_side).expect("in range"),
            "pg {major}: `{source}` normalized to `{}`, which the crate does not match",
            answer.normalized
        );
        assert_eq!(
            encode::<V>(&ours).expect("in range"),
            encode::<V>(&server_side).expect("in range"),
            "pg {major}: `{source}` encodes differently from its server-side form"
        );
    }
    Ok(())
}

/// Inserts the equality corpus and returns the id-to-spelling map.
fn load_corpus(connection: &mut PgConnection) -> Result<BTreeMap<i32, &'static str>, HarnessError> {
    let corpus = equality_corpus();
    let rows: Vec<_> = corpus
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let id = i32::try_from(index).expect("corpus is small");
            (jsonb_cases::id.eq(id), jsonb_cases::body.eq(parse(text)))
        })
        .collect();
    diesel::insert_into(jsonb_cases::table)
        .values(rows)
        .execute(connection)?;

    Ok(corpus
        .into_iter()
        .enumerate()
        .map(|(index, text)| (i32::try_from(index).expect("corpus is small"), text))
        .collect())
}

/// Shortens a spelling for an assertion message, since some are 131072 digits long.
fn elide(spelling: &str) -> String {
    if spelling.len() <= 40 {
        return spelling.to_owned();
    }
    format!(
        "{}...{} ({} chars)",
        &spelling[..20],
        &spelling[spelling.len() - 8..],
        spelling.len()
    )
}

macro_rules! major_test {
    ($name:ident, $marker:ty, $major:literal) => {
        #[test]
        fn $name() {
            run_major::<$marker>($major).expect("differential run failed");
        }
    };
}

major_test!(postgres_14, Pg14, "14");
major_test!(postgres_15, Pg15, "15");
major_test!(postgres_16, Pg16, "16");
major_test!(postgres_17, Pg17, "17");
major_test!(postgres_18, Pg18, "18");
