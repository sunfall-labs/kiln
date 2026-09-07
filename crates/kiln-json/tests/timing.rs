//! `mbx test --release -p kiln-json --test timing -- --ignored --nocapture`: the reader alone,
//! without either engine's heap, so a slow parse can be told from a slow walk; and `sizes`, the
//! reader over documents of the benchmark's shape at each size (the table simd-json was measured
//! against before it was dropped; see `src/lib.rs`).
use std::time::Instant;

fn per_parse_us(source: &[u8], budget_ms: u64) -> f64 {
    let mut iterations = 0u32;
    let mut sink = 0usize;
    let started = Instant::now();
    while started.elapsed().as_millis() < u128::from(budget_ms) || iterations < 5 {
        sink += kiln_json::parse(source, |tape| tape.nodes().len()).expect("parses");
        iterations += 1;
    }
    assert!(sink > 0);
    started.elapsed().as_secs_f64() * 1e6 / f64::from(iterations)
}

fn document(rows: usize) -> String {
    let accent = "\u{e9}l\u{e8}ve";
    let mut out = String::from("[");
    for i in 0..rows {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"id":{i},"name":"user-{i}-{accent}","email":"user{i}@example.com","score":{},"active":{},"tags":["alpha","beta","gamma-{}"],"address":{{"city":"Springfield","zip":{},"geo":[{},{}]}},"note":null}}"#,
            i as f64 * 1.5 + 0.25,
            i % 2 == 0,
            i % 7,
            10000 + i % 900,
            40.7 + i as f64 / 1e6,
            -74.0 - i as f64 / 1e6
        ));
    }
    out.push(']');
    out
}

const SMALL: &[u8] = br#"{"id":1042,"name":"kiln-benchmark","active":true,"scores":[98.5,87.2,94,100,76.8],"metadata":{"region":"us-east-1","replicas":4,"tags":["prod","edge","wasm"]}}"#;

#[test]
#[ignore]
fn reader_alone() {
    let large = document(10_000);
    for (label, source) in [("small", SMALL), ("large", large.as_bytes())] {
        let per = per_parse_us(source, 800);
        println!("{label}: {per:.3} us per parse ({} bytes)", source.len());
    }
}

/// The reader at each size of the benchmark's shape.
#[test]
#[ignore]
fn sizes() {
    let mut docs: Vec<(String, Vec<u8>)> = vec![("small.mjs payload".into(), SMALL.to_vec())];
    for target in [
        128usize, 512, 2048, 8192, 32768, 131072, 262144, 524288, 1048576, 2097152, 4194304,
        8388608,
    ] {
        // Rows are ~224 bytes each; take the row count whose document is nearest the target.
        let rows = (target / 224).max(1);
        docs.push((format!("~{target} B"), document(rows).into_bytes()));
    }
    println!("{:<20} {:>8} {:>12}", "document", "bytes", "us per parse");
    for (label, source) in &docs {
        let per = per_parse_us(source, 600);
        println!("{label:<20} {:>8} {per:>12.3}", source.len());
    }
}
