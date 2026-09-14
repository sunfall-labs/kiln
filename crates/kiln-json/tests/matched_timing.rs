use kiln_string::StringUnits;
use std::hint::black_box;
use std::time::Instant;

fn run(label: &str, source: &[u8]) {
    let mut checksum = 0usize;
    let started = Instant::now();
    let mut parses = 0usize;
    while started.elapsed().as_millis() < 700 || parses < 20_000 {
        for _ in 0..1000 {
            checksum = black_box(checksum.wrapping_add(
                kiln_json::parse(source, |tape| tape.nodes().len()).expect("parses"),
            ));
        }
        parses += 1000;
    }
    let us = started.elapsed().as_secs_f64() * 1e6 / parses as f64;
    println!(
        "{label:<14} {us:>10.4} us/parse checksum={} parses={parses}",
        checksum / parses
    );
}

fn run_units(label: &str, source: &StringUnits) {
    let mut checksum = 0usize;
    let started = Instant::now();
    let mut parses = 0usize;
    while started.elapsed().as_millis() < 700 || parses < 20_000 {
        for _ in 0..1000 {
            checksum = black_box(checksum.wrapping_add(
                kiln_json::parse_units(source, |tape| tape.nodes().len()).expect("parses"),
            ));
        }
        parses += 1000;
    }
    let us = started.elapsed().as_secs_f64() * 1e6 / parses as f64;
    println!(
        "{label:<14} {us:>10.4} us/parse checksum={} parses={parses}",
        checksum / parses
    );
}

#[test]
#[ignore = "reserved release benchmark; run explicitly with --ignored"]
fn matched() {
    let long = format!("\"{}\"", "a".repeat(4094));
    let numbers = format!(
        "[{}]",
        (0..200)
            .map(|i| format!("{}.{}e{}", i, i % 10, i % 8))
            .collect::<Vec<_>>()
            .join(",")
    );
    let cases: [(&str, Vec<u8>); 5] = [
        ("short-ascii", b"[1,2,3]".to_vec()),
        ("long-ascii", long.into_bytes()),
        ("escaped", br#"["a\u0041\u0000\ud83d\ude00"]"#.to_vec()),
        (
            "unicode",
            "[\"café 😀 café 😀 café 😀\"]".as_bytes().to_vec(),
        ),
        ("number-heavy", numbers.into_bytes()),
    ];
    for (label, source) in cases {
        run(label, &source);
    }
    run_units(
        "existing-units",
        &StringUnits::from("[\"café 😀 café 😀 café 😀\"]"),
    );
}
