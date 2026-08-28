//! Allocation counts, which Criterion cannot measure.
//!
//! One test, because only one `dhat` profiler can be live at a time.

use postgres_jsonb_canonical::{encode_into, equivalent, Pg18};
use serde_json::{Map, Value};

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("test input is valid JSON")
}

/// Runs `work` and reports how many heap blocks it allocated.
fn blocks(work: impl FnOnce()) -> u64 {
    let before = dhat::HeapStats::get().total_blocks;
    work();
    dhat::HeapStats::get().total_blocks - before
}

#[test]
fn allocation_counts_are_what_the_design_claims() {
    let _profiler = dhat::Profiler::builder().testing().build();

    // Large enough that the output buffer never grows during a measurement, so every
    // block counted below comes from the encoder rather than from the caller's buffer.
    let mut output = Vec::with_capacity(64 << 20);
    let mut encode = |value: &Value| {
        output.clear();
        encode_into::<Pg18>(value, &mut output).expect("in range");
    };

    // Scalars into a buffer that is already large enough: nothing on the heap. The object
    // scratch buffer starts empty and `Vec::new` does not allocate.
    for text in [
        "null",
        "true",
        "false",
        "1",
        "1.000000000000000000000",
        r#""hello""#,
    ] {
        let value = parse(text);
        assert_eq!(blocks(|| encode(&value)), 0, "encoding {text} allocated");
    }

    // A number is normalized in place, borrowing slices of the original text.
    let long = parse(&format!("{}.{}", "9".repeat(4096), "9".repeat(4096)));
    assert_eq!(
        blocks(|| encode(&long)),
        0,
        "normalizing a long number allocated"
    );

    // Arrays never need scratch space.
    let array = Value::Array((0..4096).map(|index| parse(&index.to_string())).collect());
    assert_eq!(blocks(|| encode(&array)), 0, "encoding an array allocated");

    // Objects sort their keys in one scratch buffer shared by the whole document.
    let wide = Value::Object(
        (0..4096)
            .map(|index| (format!("key{index:08}"), parse("1")))
            .collect::<Map<_, _>>(),
    );
    let growth = blocks(|| encode(&wide));
    assert!(
        growth <= 16,
        "scratch growth took {growth} allocations for one 4096-key object"
    );

    // The point of sharing: a hundred sibling objects cost what one costs, because each
    // truncates the buffer back on the way out and the next reuses the capacity.
    let siblings = Value::Array(vec![wide.clone(); 100]);
    let many = blocks(|| encode(&siblings));
    assert_eq!(
        many, growth,
        "100 sibling objects cost {many} against {growth} for one"
    );

    // Nesting is the case that genuinely needs more room, since every level on the path
    // holds its pairs at once. It still stays within a few doublings.
    let mut nested = wide.clone();
    for _ in 0..4 {
        let mut level = Map::new();
        level.insert("inner".to_owned(), nested);
        nested = Value::Object(level);
    }
    let deep = blocks(|| encode(&nested));
    assert!(deep <= 16, "five nested objects took {deep} allocations");

    // Comparison allocates nothing at all, at any shape.
    for (left, right) in [
        (&array, &array),
        (&wide, &wide),
        (&nested, &nested),
        (&array, &wide),
        (&long, &parse("1")),
    ] {
        let count = blocks(|| {
            equivalent::<Pg18>(left, right).expect("in range");
        });
        assert_eq!(count, 0, "comparison allocated");
    }
}
