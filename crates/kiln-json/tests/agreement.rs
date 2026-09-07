//! The reader's edge-case suite: every input the crate, `json-tape-edges.js` and
//! `json-fast-path-edges.js` care about, plus a generated sweep, each either parsing to a
//! well-formed tape or refused; and the number rule pinned to `f64::from_str` -- the fast path
//! in `number_from_parts` is a shortcut to the same answer, and this is what says so.
use kiln_json::{TAG_ARRAY, TAG_NUMBER, TAG_OBJECT, TAG_STRING};

/// Parses, and if it parses checks the tape is well-formed: every string reads back as UTF-8,
/// every container's children sum to the tape, and the walk ends exactly at the last node.
fn well_formed(source: &[u8]) -> Result<usize, kiln_json::ParseError> {
    kiln_json::parse(source, |tape| {
        let nodes = tape.nodes();
        for node in nodes {
            if node.tag == TAG_STRING {
                assert!(
                    tape.string(node).is_some(),
                    "string node reads back for {:?}",
                    String::from_utf8_lossy(source)
                );
            }
        }
        // Walk with counts: an object's `len` is pairs, an array's is elements.
        let mut stack: Vec<usize> = Vec::new();
        let mut index = 0;
        let mut root_seen = false;
        while index < nodes.len() {
            assert!(
                !root_seen,
                "nodes after the root closed in {:?}",
                String::from_utf8_lossy(source)
            );
            let node = &nodes[index];
            index += 1;
            let opened = match node.tag {
                TAG_OBJECT => Some(2 * node.len as usize),
                TAG_ARRAY => Some(node.len as usize),
                _ => None,
            };
            match opened {
                Some(0) | None => {}
                Some(children) => {
                    stack.push(children);
                    continue;
                }
            }
            loop {
                let Some(remaining) = stack.last_mut() else {
                    root_seen = true;
                    break;
                };
                *remaining -= 1;
                if *remaining != 0 {
                    break;
                }
                stack.pop();
            }
        }
        assert!(
            root_seen && stack.is_empty(),
            "tape ends inside a container for {:?}",
            String::from_utf8_lossy(source)
        );
        nodes.len()
    })
}

const PARSES: &[&str] = &[
    "null",
    "true",
    "false",
    "1",
    "-1",
    "0",
    "-0",
    "0.0",
    "-0.0",
    "[-0]",
    "1.5",
    "-1.5e2",
    " -1.5e2 ",
    "1e400",
    "-1e400",
    "[1e400,\"1e400\",2]",
    "{\"1e400\":1e-400}",
    "1e-400",
    "9007199254740993",
    "18446744073709551616",
    "-9223372036854775809",
    "123456789012345678901234567890",
    "9223372036854775807",
    "9223372036854775808",
    "-9223372036854775808",
    "18446744073709551615",
    "\"\\ud83d\\ude00\"",
    "\"a\\ud83d\\ude00b\"",
    "\"\\u0041\\u0042\"",
    "[\"\\ud83d\\ude00\", \"\\u00e9\"]",
    "{\"\\ud83d\\ude00\":1}",
    "\"\\ud83d\"",
    "\"\\ude00\"",
    "\"\\ud83dx\"",
    "\"\\ud83d\\ud83d\"",
    "\"\\ud83d\\u0041\"",
    "\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"",
    "\"\\u0000\"",
    "\"a\\u0000b\"",
    "{\"a\\u0000b\":1}",
    "{\"b\":1,\"a\":2,\"b\":3,\"__proto__\":{\"x\":1},\"c\":[]}",
    " \t\r\n{\"a\":[],\"b\":{},\"c\":[[]],\"d\":[{}]} \t\r\n",
    "[]",
    "{}",
    "[[]]",
    "[{}]",
    "{\"a\":{}}",
    "[1,2]",
    "[\"a\",\"b\"]",
    "{\"a\":1}",
    "{\"a\":1,\"b\":[2,\"x\"],\"c\":{\"d\":null}}",
    "[[1],[2,[3]]]",
    "1e21",
    "1e-7",
    "0.1",
    "123456789012345680000",
    "5e-324",
    "1.7976931348623157e308",
    "1e5",
    "1e-5",
    "0.30000000000000004",
    "42",
    "-42",
    "9007199254740991",
    "-9007199254740991",
    "9007199254740992",
    "100",
    "1000000000000000",
    "1e+5",
    "1E5",
    "1e22",
    "1e23",
    "-1e22",
    "-1e23",
    "1.5e22",
    "1.5e23",
    "123456789012345",
    "1234567890123456",
    "12345678901234567",
    "0.000000000000000000001",
    "4.9e-324",
    "2.2250738585072014e-308",
    "2.2250738585072011e-308",
    "1.7976931348623158e308",
    "1.7976931348623159e308",
    "{\"a\":1,\"b\":2,\"a\":3}",
    "{\"b\":1,\"2\":1,\"a\":1,\"1\":1}",
    "\"0123456789012345678901234567890123456789012345678901234567890123\"",
    "\"01234567890123456789012345678901234567890123456789012345678901234\"",
    "\"012345678901234567890123456789012345678901234567890123456789012\\n\"",
    "\"01234567890123456789012345678901234567890123456789012345678901\\u00e9\"",
    "[\"012345678901234567890123456789012345678901234567890123456789\",\"\\ud83d\\ude00\"]",
    "\"\u{7f}\"",
    "\"\u{e9}\u{1F600}\"",
];

const REFUSED: &[&str] = &[
    "",
    "   ",
    "1 2",
    "\u{a0}1",
    "\u{feff}1",
    "[1,]",
    "{'a':1}",
    "[truex]",
    "01e400",
    "1e400x",
    "+1",
    "\"\u{1}\"",
    "{",
    "[",
    "\"",
    "\"abc",
    "[1",
    "{\"a\"",
    "{\"a\":",
    "{\"a\":1",
    "tru",
    "nul",
    "-",
    "1.",
    ".5",
    "1e",
    "1e+",
    "0x10",
    "[1 2]",
    "{\"a\" 1}",
    "{1:2}",
    "\"\\x\"",
    "\"\\u12\"",
    "\"\\uZZZZ\"",
    "\"\\\"",
    "01",
    "-01",
    "1.e5",
    "1e5.5",
    "[,1]",
    "{,}",
    "{\"a\":1,}",
    "[1,,2]",
    "\"\t\"",
    "\"\n\"",
    "nulll",
    "truefalse",
    "[]x",
    "{}{}",
    "\"a\"\"b\"",
    "Infinity",
    "NaN",
    "-Infinity",
    "undefined",
    "'a'",
];

#[test]
fn every_edge_input_parses_to_a_well_formed_tape() {
    for input in PARSES {
        assert!(
            well_formed(input.as_bytes()).is_ok(),
            "{input:?} should parse"
        );
    }
}

#[test]
fn every_refusal_is_refused() {
    for input in REFUSED {
        assert!(
            well_formed(input.as_bytes()).is_err(),
            "{input:?} should be refused"
        );
    }
}

#[test]
fn the_edge_probe_documents_parse() {
    let deep = format!("{}{}", "[".repeat(3000), "]".repeat(3000));
    assert_eq!(well_formed(deep.as_bytes()), Ok(3000));
    let deep_objects = format!("{}1{}", "{\"a\":".repeat(2000), "}".repeat(2000));
    assert_eq!(well_formed(deep_objects.as_bytes()), Ok(4001));
    let mut many = String::from("{");
    for i in 0..40 {
        if i > 0 {
            many.push(',');
        }
        many.push_str(&format!("\"k{i}\":{i}"));
    }
    many.push('}');
    assert_eq!(well_formed(many.as_bytes()), Ok(81));
}

#[test]
fn a_generated_sweep_of_shapes_parses() {
    let escapes = [
        "\\\"",
        "\\\\",
        "\\/",
        "\\b",
        "\\f",
        "\\n",
        "\\r",
        "\\t",
        "\\u0000",
        "\\u001f",
        "\\u0041",
        "\\u00e9",
        "\\u20ac",
        "\\ud83d\\ude00",
        "\\ud83d",
        "\\ude00",
        "\\ud83d\\u0041",
        "\\ude00\\ud83d",
        "\\ud800\\udc00",
        "\\udbff\\udfff",
    ];
    for escape in escapes {
        for source in [
            format!("\"{escape}\""),
            format!("\"ab{escape}cd\""),
            format!("{{\"{escape}\":\"{escape}\"}}"),
            format!("[\"{escape}\",1,\"x{escape}\"]"),
        ] {
            assert!(well_formed(source.as_bytes()).is_ok(), "{source:?}");
        }
    }
    for depth in [1usize, 2, 3, 7, 33, 64, 65] {
        for source in [
            format!("{}1{}", "[".repeat(depth), "]".repeat(depth)),
            format!("{}[]{}", "{\"k\":".repeat(depth), "}".repeat(depth)),
            format!(
                "{}{{\"a\":[true,null,false,{{}}]}}{}",
                "[{\"x\":".repeat(depth),
                "}]".repeat(depth)
            ),
        ] {
            assert!(well_formed(source.as_bytes()).is_ok(), "{source:?}");
        }
    }
    for len in [60usize, 62, 63, 64, 65, 66, 100, 127, 128, 129, 200, 1000] {
        let plain = "a".repeat(len);
        let accented = "\u{e9}".repeat(len / 2);
        for source in [
            format!("\"{plain}\""),
            format!("[\"{plain}\",\"{plain}\\n\",{{\"{plain}\":\"\\u00e9{plain}\"}}]"),
            format!("\"{accented}\""),
        ] {
            assert!(well_formed(source.as_bytes()).is_ok(), "{source:?}");
        }
    }
    for rows in [1usize, 4, 16, 64, 256] {
        assert!(well_formed(document(rows).as_bytes()).is_ok());
    }
}

/// The number a document `[n]` parses to.
fn number(text: &str) -> f64 {
    kiln_json::parse(format!("[{text}]").as_bytes(), |tape| {
        let node = &tape.nodes()[1];
        assert_eq!(node.tag, TAG_NUMBER, "{text}");
        node.as_number().unwrap_or(f64::NAN)
    })
    .unwrap_or_else(|error| panic!("{text}: {error:?}"))
}

fn pin(text: &str) {
    let expected: f64 = text
        .parse()
        .unwrap_or_else(|error| panic!("{text}: {error}"));
    let actual = number(text);
    assert!(
        actual.to_bits() == expected.to_bits(),
        "{text}: reader {actual:?} ({:#x}) vs from_str {expected:?} ({:#x})",
        actual.to_bits(),
        expected.to_bits()
    );
}

#[test]
fn numbers_agree_with_from_str_across_a_sweep() {
    // Every significant-digit count the fast path takes and the first two it does not, at every
    // exponent from -30 to 30 including the +/-22 edge, with and without a fraction, both signs.
    let mantissas: Vec<String> = (1..=17)
        .flat_map(|digits| {
            let ones = "1".repeat(digits);
            let nines = "9".repeat(digits);
            let mixed: String = (0..digits)
                .map(|i| char::from(b'1' + (i % 9) as u8))
                .collect();
            [ones, nines, mixed]
        })
        .collect();
    for mantissa in &mantissas {
        for exponent in -30..=30 {
            pin(&format!("{mantissa}e{exponent}"));
            pin(&format!("-{mantissa}E{exponent:+}"));
            if mantissa.len() > 1 {
                let (head, tail) = mantissa.split_at(1);
                pin(&format!("{head}.{tail}e{exponent}"));
                pin(&format!("-{head}.{tail}e{exponent}"));
            }
        }
    }
    // -0 in every spelling, the 2^53 neighbourhood, the exact-power edge, and past u64.
    for text in [
        "-0", "-0.0", "-0e0", "-0.0e-5", "0", "0.0", "0e400", "-0e400",
    ] {
        pin(text);
    }
    for base in 9007199254740985u64..=9007199254740999 {
        pin(&base.to_string());
        pin(&format!("-{base}"));
        pin(&format!("{base}.5"));
        pin(&format!("{base}e0"));
        pin(&format!("{base}e1"));
    }
    for text in [
        "1e22",
        "1e23",
        "-1e22",
        "-1e23",
        "9007199254740993e22",
        "9007199254740993e23",
        "18446744073709551616",
        "18446744073709551615",
        "18446744073709551617",
        "-18446744073709551616",
        "123456789012345678901234567890",
        "0.000000000000000000001",
        "1e-22",
        "1e-23",
        "999999999999999e22",
        "999999999999999e23",
        "9999999999999999e22",
        "1e308",
        "1e309",
        "1e-324",
        "1e-325",
        "2.2250738585072011e-308",
        "4.9e-324",
        "1.7976931348623157e308",
        "1.7976931348623159e308",
        "5e-324",
        "2.5e-324",
        "7.4e-324",
    ] {
        pin(text);
    }
    // Sanity on the documents: `1e400` is infinity and `-0` keeps its sign through the tape.
    assert_eq!(number("1e400"), f64::INFINITY);
    assert!(number("-0").is_sign_negative());
    let _ = (TAG_ARRAY, TAG_OBJECT);
}

fn document(rows: usize) -> String {
    let mut out = String::from("[");
    for i in 0..rows {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"id":{i},"name":"user-{i}-\u00e9l\u00e8ve","email":"user{i}@example.com","score":{},"active":{},"tags":["alpha","beta","gamma-{}"],"address":{{"city":"Springfield","zip":{},"geo":[{},{}]}},"note":null}}"#,
            i as f64 * 1.5 + 0.25, i % 2 == 0, i % 7, 10000 + i % 900, 40.7 + i as f64 / 1e6, -74.0 - i as f64 / 1e6
        ));
    }
    out.push(']');
    out
}
