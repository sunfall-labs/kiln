//! Each ADR number names one decision.
//!
//! Two ADRs were numbered 0059 on 2026-09-13 because two sessions each took "the next number" from
//! their own checkout, and the collision stood for three weeks; a reference to "ADR 0059" could
//! have meant either. `scripts/new-adr.sh` picks the number from the checkout and origin/main
//! together, and this test is what fails if a collision lands anyway.

use std::collections::BTreeMap;
use std::path::Path;

#[test]
fn adr_numbers_are_unique() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/adr");
    let mut by_number: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in std::fs::read_dir(&directory).expect("docs/adr/ exists") {
        let name = entry
            .expect("docs/adr/ entry is readable")
            .file_name()
            .to_string_lossy()
            .into_owned();
        let Some((number, _)) = name.split_once('-') else {
            continue;
        };
        if number.len() == 4 && number.bytes().all(|byte| byte.is_ascii_digit()) {
            by_number.entry(number.to_owned()).or_default().push(name);
        }
    }
    assert!(
        by_number.len() > 50,
        "found only {} numbered ADRs; this guard would pass vacuously",
        by_number.len()
    );
    let duplicates: Vec<_> = by_number.values().filter(|names| names.len() > 1).collect();
    assert!(
        duplicates.is_empty(),
        "ADR numbers used more than once: {duplicates:?}. Renumber the one fewer documents refer \
         to, with a status line saying so (see ADR 0076), and use scripts/new-adr.sh next time."
    );
}
