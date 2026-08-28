//! Criterion baselines for encoding and comparison.
//!
//! Inputs are preparsed values built outside the timed closure, so parser cost never hides
//! canonicalization cost. Parsing has its own group for exactly that reason.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use postgres_jsonb_canonical::{encode, encode_into, equivalent, Pg18};
use serde_json::{Map, Value};

const WIDTHS: [usize; 5] = [0, 1, 16, 256, 4096];
const OBJECT_WIDTHS: [usize; 5] = [1, 8, 64, 512, 4096];
const DEPTHS: [usize; 4] = [1, 8, 32, 127];

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("benchmark input is valid JSON")
}

fn array(width: usize) -> Value {
    Value::Array((0..width).map(|index| parse(&index.to_string())).collect())
}

/// Keys all the same length, so ordering never has to fall back to comparing bytes.
fn object_equal_length_keys(width: usize) -> Value {
    Value::Object(
        (0..width)
            .map(|index| (format!("key{index:08}"), parse("1")))
            .collect::<Map<_, _>>(),
    )
}

/// Key lengths spread across the whole range, which is the adversarial case for an ordering
/// that compares length first.
fn object_varied_length_keys(width: usize) -> Value {
    Value::Object(
        (0..width)
            .map(|index| {
                let length = 1 + index % 24;
                (
                    format!("{index:0length$}", length = length.max(1)),
                    parse("1"),
                )
            })
            .collect::<Map<_, _>>(),
    )
}

/// One container per level, all the way down.
fn chain(depth: usize) -> Value {
    let mut value = parse("1");
    for _ in 0..depth {
        value = Value::Array(vec![value]);
    }
    value
}

/// A balanced tree with roughly the same node count at each depth.
fn balanced(depth: usize) -> Value {
    let mut value = parse("1");
    for _ in 0..depth {
        value = Value::Array(vec![value.clone(), value]);
    }
    value
}

fn source_bytes(value: &Value) -> u64 {
    u64::try_from(value.to_string().len()).expect("benchmark input is small")
}

fn scalars(criterion: &mut Criterion) {
    let long_string = Value::String("x".repeat(4096));
    let long_number = parse(&"9".repeat(1000));
    let cases: [(&str, Value); 8] = [
        ("null", parse("null")),
        ("bool", parse("true")),
        ("string_short", parse(r#""hello""#)),
        ("string_long", long_string),
        ("number_small", parse("1")),
        ("number_trailing_zeros", parse("1.000000000000000000000")),
        ("number_25_digits", parse("12345678901234567890123456")),
        ("number_1000_digits", long_number),
    ];

    let mut group = criterion.benchmark_group("encode/scalars");
    for (name, value) in &cases {
        group.throughput(Throughput::Bytes(source_bytes(value)));
        group.bench_function(*name, |bencher| {
            bencher.iter(|| encode::<Pg18>(black_box(value)).expect("in range"));
        });
    }
    group.finish();
}

fn arrays(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("encode/arrays");
    for width in WIDTHS {
        let value = array(width);
        group.throughput(Throughput::Elements(u64::try_from(width).expect("small")));
        group.bench_with_input(
            BenchmarkId::from_parameter(width),
            &value,
            |bencher, value| {
                bencher.iter(|| encode::<Pg18>(black_box(value)).expect("in range"));
            },
        );
    }
    group.finish();
}

fn objects(criterion: &mut Criterion) {
    for (name, build) in [
        (
            "encode/objects_equal_length_keys",
            object_equal_length_keys as fn(usize) -> Value,
        ),
        (
            "encode/objects_varied_length_keys",
            object_varied_length_keys,
        ),
    ] {
        let mut group = criterion.benchmark_group(name);
        for width in OBJECT_WIDTHS {
            let value = build(width);
            group.throughput(Throughput::Elements(u64::try_from(width).expect("small")));
            group.bench_with_input(
                BenchmarkId::from_parameter(width),
                &value,
                |bencher, value| {
                    bencher.iter(|| encode::<Pg18>(black_box(value)).expect("in range"));
                },
            );
        }
        group.finish();
    }
}

fn nested(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("encode/nested");
    for depth in DEPTHS {
        let single = chain(depth);
        group.bench_with_input(
            BenchmarkId::new("chain", depth),
            &single,
            |bencher, value| {
                bencher.iter(|| encode::<Pg18>(black_box(value)).expect("in range"));
            },
        );
    }
    // A balanced tree doubles in size per level, so it stops well before the depth cap.
    for depth in [1usize, 4, 8, 12] {
        let tree = balanced(depth);
        group.bench_with_input(
            BenchmarkId::new("balanced", depth),
            &tree,
            |bencher, value| {
                bencher.iter(|| encode::<Pg18>(black_box(value)).expect("in range"));
            },
        );
    }
    group.finish();
}

/// The design the crate rejected: stop at the first difference, validate nothing.
///
/// Present so the cost of strictness is measured rather than assumed. It compares numbers
/// by their text alone, which a real short-circuiting implementation could not do, so it is
/// faster than the honest version and therefore biased against the design that shipped.
fn short_circuit(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_str() == right.as_str(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| short_circuit(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| short_circuit(left, right))
                })
        }
        (left, right) => left == right,
    }
}

fn comparison(criterion: &mut Criterion) {
    let width = 4096;
    let base = array(width);

    let mut early = match base.clone() {
        Value::Array(values) => values,
        other => vec![other],
    };
    let mut late = early.clone();
    early[0] = parse("-1");
    late[width - 1] = parse("-1");
    let (early, late) = (Value::Array(early), Value::Array(late));

    let mut group = criterion.benchmark_group("equivalent");
    group.throughput(Throughput::Elements(u64::try_from(width).expect("small")));
    group.bench_function("equal", |bencher| {
        bencher.iter(|| equivalent::<Pg18>(black_box(&base), black_box(&base)).expect("in range"));
    });
    group.bench_function("early_mismatch", |bencher| {
        bencher.iter(|| equivalent::<Pg18>(black_box(&base), black_box(&early)).expect("in range"));
    });
    group.bench_function("late_mismatch", |bencher| {
        bencher.iter(|| equivalent::<Pg18>(black_box(&base), black_box(&late)).expect("in range"));
    });
    // The rejected short-circuit design, on the same inputs. On an early mismatch it exits
    // at once where the shipped one still walks both values, which is the whole cost of
    // refusing to let an unstorable number slip past.
    group.bench_function("early_mismatch_short_circuit", |bencher| {
        bencher.iter(|| short_circuit(black_box(&base), black_box(&early)));
    });
    group.bench_function("late_mismatch_short_circuit", |bencher| {
        bencher.iter(|| short_circuit(black_box(&base), black_box(&late)));
    });
    // What the plan warns against: comparing by building both encodings.
    group.bench_function("equal_via_encoding", |bencher| {
        bencher.iter(|| {
            encode::<Pg18>(black_box(&base)).expect("in range")
                == encode::<Pg18>(black_box(&base)).expect("in range")
        });
    });
    group.finish();
}

fn numbers(criterion: &mut Criterion) {
    let long_mantissa = parse(&format!("1.{}", "2".repeat(500)));
    let heavy_trailing = parse(&format!("1.{}", "0".repeat(4096)));
    let cases: [(&str, Value); 6] = [
        ("integer", parse("123456")),
        ("fractional", parse("123.456")),
        ("exponent", parse("1.23456e300")),
        ("long_mantissa", long_mantissa),
        ("trailing_zero_heavy", heavy_trailing),
        ("zero", parse("-0.000")),
    ];

    let mut group = criterion.benchmark_group("numbers");
    for (name, value) in &cases {
        group.throughput(Throughput::Bytes(source_bytes(value)));
        group.bench_function(*name, |bencher| {
            bencher.iter(|| encode::<Pg18>(black_box(value)).expect("in range"));
        });
    }
    group.finish();
}

fn output_reuse(criterion: &mut Criterion) {
    let value = array(256);
    let mut group = criterion.benchmark_group("output_reuse");
    group.throughput(Throughput::Elements(256));
    group.bench_function("fresh_vec", |bencher| {
        bencher.iter(|| encode::<Pg18>(black_box(&value)).expect("in range"));
    });
    group.bench_function("cleared_vec", |bencher| {
        let mut output = Vec::with_capacity(8192);
        bencher.iter(|| {
            output.clear();
            encode_into::<Pg18>(black_box(&value), &mut output).expect("in range");
        });
    });
    group.finish();
}

/// Parsing, kept apart so it never hides the cost of the work above.
fn parsing(criterion: &mut Criterion) {
    let sources = [
        ("array_4096", array(4096).to_string()),
        ("object_512", object_equal_length_keys(512).to_string()),
    ];
    let mut group = criterion.benchmark_group("parse");
    for (name, text) in &sources {
        group.throughput(Throughput::Bytes(u64::try_from(text.len()).expect("small")));
        group.bench_function(*name, |bencher| {
            bencher.iter(|| serde_json::from_str::<Value>(black_box(text)).expect("valid"));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    scalars,
    arrays,
    objects,
    nested,
    comparison,
    numbers,
    output_reuse,
    parsing
);
criterion_main!(benches);
