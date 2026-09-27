//! Deterministic changed-test selection for Kiln's compiler and conformance suites.
//!
//! A test's metadata describes the compiler surface it exercises.  A [`ChangeSet`] describes
//! the surface touched by an edit.  [`select`] then returns failed tests first, universal smoke
//! tests second, and tests affected by the change third.  The selector owns all of its strings
//! and uses ordered collections, so selection does not depend on filesystem or registration
//! order.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A stable, owned identifier for a test.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TestId(String);

impl TestId {
    /// Construct an identifier from a path or another stable test name.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl<S: Into<String>> From<S> for TestId {
    fn from(value: S) -> Self {
        Self::new(value)
    }
}

impl From<&TestId> for TestId {
    fn from(value: &TestId) -> Self {
        value.clone()
    }
}

impl AsRef<str> for TestId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for TestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Ordered, owned metadata tags attached to a test.
pub type Tags = BTreeSet<String>;

/// Local test execution tiers from design.md §9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TestTier {
    /// Unit and universal smoke checks.
    #[default]
    Tier0,
    /// Affected regressions.
    Tier1,
    /// Affected Test262/Node conformance tests.
    Tier2,
    /// Native-equivalence checks.
    Tier3,
    /// Complete suites.
    Tier4,
}

impl TestTier {
    pub const fn number(self) -> u8 {
        match self {
            Self::Tier0 => 0,
            Self::Tier1 => 1,
            Self::Tier2 => 2,
            Self::Tier3 => 3,
            Self::Tier4 => 4,
        }
    }
}

/// Metadata describing the compiler and runtime surface exercised by one test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestMetadata {
    pub id: TestId,
    pub compiler_crates: Tags,
    pub features: Tags,
    pub ir_operations: Tags,
    pub builtin_modules: Tags,
    pub host_ops: Tags,
    pub smoke: bool,
    pub tier: TestTier,
}

impl TestMetadata {
    pub fn new(id: impl Into<TestId>) -> Self {
        Self {
            id: id.into(),
            compiler_crates: Tags::new(),
            features: Tags::new(),
            ir_operations: Tags::new(),
            builtin_modules: Tags::new(),
            host_ops: Tags::new(),
            smoke: false,
            tier: TestTier::Tier0,
        }
    }

    pub fn smoke(mut self) -> Self {
        self.smoke = true;
        self
    }

    pub fn with_smoke(self, smoke: bool) -> Self {
        Self { smoke, ..self }
    }

    pub fn with_tier(self, tier: TestTier) -> Self {
        Self { tier, ..self }
    }

    pub fn compiler_crate(mut self, tag: impl Into<String>) -> Self {
        self.compiler_crates.insert(tag.into());
        self
    }

    pub fn feature(mut self, tag: impl Into<String>) -> Self {
        self.features.insert(tag.into());
        self
    }

    pub fn ir_operation(mut self, tag: impl Into<String>) -> Self {
        self.ir_operations.insert(tag.into());
        self
    }

    pub fn builtin_module(mut self, tag: impl Into<String>) -> Self {
        self.builtin_modules.insert(tag.into());
        self
    }

    pub fn host_op(mut self, tag: impl Into<String>) -> Self {
        self.host_ops.insert(tag.into());
        self
    }

    pub fn add_compiler_crate(&mut self, tag: impl Into<String>) {
        self.compiler_crates.insert(tag.into());
    }

    pub fn add_feature(&mut self, tag: impl Into<String>) {
        self.features.insert(tag.into());
    }

    pub fn add_ir_operation(&mut self, tag: impl Into<String>) {
        self.ir_operations.insert(tag.into());
    }

    pub fn add_builtin_module(&mut self, tag: impl Into<String>) {
        self.builtin_modules.insert(tag.into());
    }

    pub fn add_host_op(&mut self, tag: impl Into<String>) {
        self.host_ops.insert(tag.into());
    }
}

/// Compiler and runtime surfaces changed by one edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    pub compiler_crates: Tags,
    pub features: Tags,
    pub ir_operations: Tags,
    pub builtin_modules: Tags,
    pub host_ops: Tags,
    /// Tests whose source files were edited directly.
    pub changed_tests: BTreeSet<TestId>,
    /// Select every known test, while retaining ordinary priority ordering.
    pub full_run: bool,
}

impl ChangeSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn full_run() -> Self {
        Self {
            full_run: true,
            ..Self::default()
        }
    }

    pub fn force_full_run(&mut self) {
        self.full_run = true;
    }

    pub fn is_full_run(&self) -> bool {
        self.full_run
    }

    pub fn is_empty(&self) -> bool {
        !self.full_run
            && self.compiler_crates.is_empty()
            && self.features.is_empty()
            && self.ir_operations.is_empty()
            && self.builtin_modules.is_empty()
            && self.host_ops.is_empty()
            && self.changed_tests.is_empty()
    }

    pub fn compiler_crate(mut self, tag: impl Into<String>) -> Self {
        self.compiler_crates.insert(tag.into());
        self
    }

    pub fn ir_operation(mut self, tag: impl Into<String>) -> Self {
        self.ir_operations.insert(tag.into());
        self
    }

    pub fn feature(mut self, tag: impl Into<String>) -> Self {
        self.features.insert(tag.into());
        self
    }

    pub fn builtin_module(mut self, tag: impl Into<String>) -> Self {
        self.builtin_modules.insert(tag.into());
        self
    }

    pub fn host_op(mut self, tag: impl Into<String>) -> Self {
        self.host_ops.insert(tag.into());
        self
    }

    pub fn changed_test(mut self, id: impl Into<TestId>) -> Self {
        self.changed_tests.insert(id.into());
        self
    }

    pub fn add_compiler_crate(&mut self, tag: impl Into<String>) {
        self.compiler_crates.insert(tag.into());
    }

    pub fn add_ir_operation(&mut self, tag: impl Into<String>) {
        self.ir_operations.insert(tag.into());
    }

    pub fn add_feature(&mut self, tag: impl Into<String>) {
        self.features.insert(tag.into());
    }

    pub fn add_builtin_module(&mut self, tag: impl Into<String>) {
        self.builtin_modules.insert(tag.into());
    }

    pub fn add_host_op(&mut self, tag: impl Into<String>) {
        self.host_ops.insert(tag.into());
    }

    pub fn add_changed_test(&mut self, id: impl Into<TestId>) {
        self.changed_tests.insert(id.into());
    }
}

/// Why a test was selected.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SelectionReason {
    PreviouslyFailed,
    UniversalSmoke,
    ModifiedCompilerCrate(String),
    ModifiedFeature(String),
    ModifiedIrOperation(String),
    ModifiedBuiltinModule(String),
    ModifiedHostOp(String),
    ModifiedTest(TestId),
    FullRun,
}

impl SelectionReason {
    fn priority(&self) -> u8 {
        match self {
            Self::PreviouslyFailed => 0,
            Self::UniversalSmoke => 1,
            Self::ModifiedCompilerCrate(_)
            | Self::ModifiedFeature(_)
            | Self::ModifiedIrOperation(_)
            | Self::ModifiedBuiltinModule(_)
            | Self::ModifiedHostOp(_)
            | Self::ModifiedTest(_) => 2,
            Self::FullRun => 3,
        }
    }
}

/// One selected test and all reasons it was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedTest {
    pub id: TestId,
    pub reasons: Vec<SelectionReason>,
}

impl SelectedTest {
    pub fn has_reason(&self, reason: &SelectionReason) -> bool {
        self.reasons.contains(reason)
    }
}

/// Ordered selector output.  Tests are deduplicated by [`TestId`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub tests: Vec<SelectedTest>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tests.len()
    }

    pub fn ids(&self) -> impl Iterator<Item = &TestId> {
        self.tests.iter().map(|test| &test.id)
    }

    pub fn selected_tests(&self) -> &[SelectedTest] {
        &self.tests
    }

    pub fn selected(&self) -> &[SelectedTest] {
        self.selected_tests()
    }

    pub fn reasons_for(&self, id: &TestId) -> Option<&[SelectionReason]> {
        self.tests
            .iter()
            .find(|test| test.id == *id)
            .map(|test| test.reasons.as_slice())
    }
}

/// Remember tests that failed on the previous attempt.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FailureState {
    failing: BTreeSet<TestId>,
}

impl FailureState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_failing<I, T>(tests: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<TestId>,
    {
        let mut state = Self::new();
        for test in tests {
            state.record_failure(test);
        }
        state
    }

    pub fn record_failure(&mut self, id: impl Into<TestId>) {
        self.failing.insert(id.into());
    }

    pub fn record_success(&mut self, id: impl Into<TestId>) {
        self.failing.remove(&id.into());
    }

    /// Update one result. `passed = false` retains the test for the next attempt.
    pub fn update_result(&mut self, id: impl Into<TestId>, passed: bool) {
        let id = id.into();
        if passed {
            self.failing.remove(&id);
        } else {
            self.failing.insert(id);
        }
    }

    /// Alias for [`Self::update_result`].
    pub fn update(&mut self, id: impl Into<TestId>, passed: bool) {
        self.update_result(id, passed);
    }

    /// Apply a batch of `(test id, passed)` results in iteration order.
    pub fn update_from_results<I, T>(&mut self, results: I)
    where
        I: IntoIterator<Item = (T, bool)>,
        T: Into<TestId>,
    {
        for (id, passed) in results {
            self.update_result(id, passed);
        }
    }

    pub fn is_failing(&self, id: &TestId) -> bool {
        self.failing.contains(id)
    }

    pub fn contains(&self, id: &TestId) -> bool {
        self.is_failing(id)
    }

    pub fn len(&self) -> usize {
        self.failing.len()
    }

    pub fn is_empty(&self) -> bool {
        self.failing.is_empty()
    }

    pub fn failing_tests(&self) -> impl Iterator<Item = &TestId> {
        self.failing.iter()
    }

    /// Serialize using the stable `kiln-test-selector-failures-v1` line format.
    pub fn serialize(&self) -> String {
        let mut output = String::from(FAILURE_FORMAT_HEADER);
        output.push('\n');
        for id in &self.failing {
            output.push_str("failed\t");
            output.push_str(&escape_line(id.as_str()));
            output.push('\n');
        }
        output
    }

    pub fn to_text(&self) -> String {
        self.serialize()
    }

    pub fn parse(input: &str) -> Result<Self, PersistenceError> {
        let mut lines = input.lines();
        if lines.next() != Some(FAILURE_FORMAT_HEADER) {
            return Err(PersistenceError::InvalidHeader);
        }

        let mut state = Self::new();
        for (line_number, line) in lines.enumerate() {
            let line_number = line_number + 2;
            let Some(encoded) = line.strip_prefix("failed\t") else {
                return Err(PersistenceError::MalformedLine { line: line_number });
            };
            let id = unescape_line(encoded).map_err(|kind| PersistenceError::InvalidEscape {
                line: line_number,
                kind,
            })?;
            state.record_failure(TestId::new(id));
        }
        Ok(state)
    }

    pub fn from_text(input: &str) -> Result<Self, PersistenceError> {
        Self::parse(input)
    }
}

const FAILURE_FORMAT_HEADER: &str = "kiln-test-selector-failures-v1";

/// Errors returned while reading the failure-state text format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceError {
    InvalidHeader,
    MalformedLine { line: usize },
    InvalidEscape { line: usize, kind: EscapeError },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscapeError {
    Truncated,
    NonHex,
    InvalidUtf8,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader => formatter.write_str("invalid failure-state header"),
            Self::MalformedLine { line } => {
                write!(formatter, "malformed failure-state line {line}")
            }
            Self::InvalidEscape { line, kind } => {
                write!(
                    formatter,
                    "invalid escape on failure-state line {line}: {kind}"
                )
            }
        }
    }
}

impl fmt::Display for EscapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("truncated percent escape"),
            Self::NonHex => formatter.write_str("non-hex percent escape"),
            Self::InvalidUtf8 => formatter.write_str("escaped bytes are not UTF-8"),
        }
    }
}

impl std::error::Error for PersistenceError {}
impl std::error::Error for EscapeError {}

fn escape_line(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':') {
            escaped.push(char::from(*byte));
        } else {
            escaped.push('%');
            escaped.push(hex_digit(byte >> 4));
            escaped.push(hex_digit(byte & 0x0f));
        }
    }
    escaped
}

fn unescape_line(value: &str) -> Result<String, EscapeError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(EscapeError::Truncated);
        }
        let Some(high) = from_hex(bytes[index + 1]) else {
            return Err(EscapeError::NonHex);
        };
        let Some(low) = from_hex(bytes[index + 2]) else {
            return Err(EscapeError::NonHex);
        };
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| EscapeError::InvalidUtf8)
}

fn hex_digit(value: u8) -> char {
    char::from(b"0123456789ABCDEF"[usize::from(value)])
}

fn from_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Select tests from metadata and the current failure state.
pub fn select<I, M>(metadata: I, changes: &ChangeSet, failures: &FailureState) -> Selection
where
    I: IntoIterator<Item = M>,
    M: Borrow<TestMetadata>,
{
    let mut reasons_by_id: BTreeMap<TestId, BTreeSet<SelectionReason>> = BTreeMap::new();

    // Duplicate metadata is merged by ID. This makes registration order irrelevant and lets
    // independently discovered metadata contribute all of its reasons.
    let mut by_id: BTreeMap<TestId, TestMetadata> = BTreeMap::new();
    for item in metadata {
        let item = item.borrow();
        by_id
            .entry(item.id.clone())
            .and_modify(|existing| merge_metadata(existing, item))
            .or_insert_with(|| item.clone());
    }

    for (id, metadata) in &by_id {
        let mut reasons = BTreeSet::new();
        if failures.contains(id) {
            reasons.insert(SelectionReason::PreviouslyFailed);
        }
        if metadata.smoke {
            reasons.insert(SelectionReason::UniversalSmoke);
        }
        if changes.full_run {
            reasons.insert(SelectionReason::FullRun);
        } else {
            add_intersection_reasons(&mut reasons, metadata, changes);
        }
        if !reasons.is_empty() {
            reasons_by_id.insert(id.clone(), reasons);
        }
    }

    // Group by the earliest reason priority, then use the owned ID as a stable tie-breaker.
    let mut selected: Vec<_> = reasons_by_id.into_iter().collect();
    selected.sort_by(|(left_id, left_reasons), (right_id, right_reasons)| {
        reason_priority(left_reasons)
            .cmp(&reason_priority(right_reasons))
            .then_with(|| left_id.cmp(right_id))
    });

    Selection {
        tests: selected
            .into_iter()
            .map(|(id, reasons)| SelectedTest {
                id,
                reasons: reasons.into_iter().collect(),
            })
            .collect(),
    }
}

/// Alias that reads naturally at call sites which already use a selector object.
pub fn select_tests(
    metadata: &[TestMetadata],
    changes: &ChangeSet,
    failures: &FailureState,
) -> Selection {
    select(metadata, changes, failures)
}

fn merge_metadata(existing: &mut TestMetadata, incoming: &TestMetadata) {
    existing
        .compiler_crates
        .extend(incoming.compiler_crates.iter().cloned());
    existing.features.extend(incoming.features.iter().cloned());
    existing
        .ir_operations
        .extend(incoming.ir_operations.iter().cloned());
    existing
        .builtin_modules
        .extend(incoming.builtin_modules.iter().cloned());
    existing.host_ops.extend(incoming.host_ops.iter().cloned());
    existing.smoke |= incoming.smoke;
    existing.tier = existing.tier.min(incoming.tier);
}

fn add_intersection_reasons(
    reasons: &mut BTreeSet<SelectionReason>,
    metadata: &TestMetadata,
    changes: &ChangeSet,
) {
    for tag in metadata
        .compiler_crates
        .intersection(&changes.compiler_crates)
    {
        reasons.insert(SelectionReason::ModifiedCompilerCrate(tag.clone()));
    }
    for tag in metadata.features.intersection(&changes.features) {
        reasons.insert(SelectionReason::ModifiedFeature(tag.clone()));
    }
    for tag in metadata.ir_operations.intersection(&changes.ir_operations) {
        reasons.insert(SelectionReason::ModifiedIrOperation(tag.clone()));
    }
    for tag in metadata
        .builtin_modules
        .intersection(&changes.builtin_modules)
    {
        reasons.insert(SelectionReason::ModifiedBuiltinModule(tag.clone()));
    }
    for tag in metadata.host_ops.intersection(&changes.host_ops) {
        reasons.insert(SelectionReason::ModifiedHostOp(tag.clone()));
    }
    if changes.changed_tests.contains(&metadata.id) {
        reasons.insert(SelectionReason::ModifiedTest(metadata.id.clone()));
    }
}

fn reason_priority(reasons: &BTreeSet<SelectionReason>) -> u8 {
    reasons
        .iter()
        .map(SelectionReason::priority)
        .min()
        .unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(selection: &Selection) -> Vec<&str> {
        selection.ids().map(TestId::as_str).collect()
    }

    #[test]
    fn failures_then_smoke_then_directly_affected() {
        let metadata = vec![
            TestMetadata::new("z-affected").ir_operation("load_capture"),
            TestMetadata::new("a-smoke").smoke(),
            TestMetadata::new("m-failing"),
            TestMetadata::new("b-both")
                .smoke()
                .builtin_module("console"),
            TestMetadata::new("c-modified"),
            TestMetadata::new("unrelated"),
        ];
        let changes = ChangeSet::new()
            .builtin_module("console")
            .changed_test("c-modified");
        let failures = FailureState::from_failing(["m-failing", "b-both"]);
        let selection = select(metadata, &changes, &failures);

        assert_eq!(
            ids(&selection),
            ["b-both", "m-failing", "a-smoke", "c-modified"]
        );
        assert!(selection.tests[0].has_reason(&SelectionReason::PreviouslyFailed));
        assert!(selection.tests[0].has_reason(&SelectionReason::UniversalSmoke));
        assert!(
            selection.tests[0]
                .has_reason(&SelectionReason::ModifiedBuiltinModule("console".into()))
        );
        assert!(selection.tests[1].has_reason(&SelectionReason::PreviouslyFailed));
        assert!(selection.tests[2].has_reason(&SelectionReason::UniversalSmoke));
        assert!(selection.tests[3].has_reason(&SelectionReason::ModifiedTest("c-modified".into())));
    }

    #[test]
    fn directly_modified_ordinary_test_is_selected_without_selecting_unaffected_tests() {
        let metadata = vec![
            TestMetadata::new("changed.js"),
            TestMetadata::new("unaffected.js"),
        ];
        let selection = select(
            metadata,
            &ChangeSet::new().changed_test("changed.js"),
            &FailureState::new(),
        );

        assert_eq!(ids(&selection), ["changed.js"]);
        assert!(selection.tests[0].has_reason(&SelectionReason::ModifiedTest("changed.js".into())));
    }

    #[test]
    fn all_change_dimensions_select_matching_metadata() {
        let metadata = vec![
            TestMetadata::new("crate").compiler_crate("kiln-ir"),
            TestMetadata::new("feature").feature("closures"),
            TestMetadata::new("ir").ir_operation("load_capture"),
            TestMetadata::new("builtin").builtin_module("fs"),
            TestMetadata::new("host").host_op("write_stdout"),
        ];
        let changes = ChangeSet::new()
            .compiler_crate("kiln-ir")
            .feature("closures")
            .ir_operation("load_capture")
            .builtin_module("fs")
            .host_op("write_stdout");
        let selection = select_tests(&metadata, &changes, &FailureState::new());
        assert_eq!(
            ids(&selection),
            ["builtin", "crate", "feature", "host", "ir"]
        );
        assert!(selection.tests.iter().all(|test| test.reasons.len() == 1));
        assert!(
            selection.tests[2].has_reason(&SelectionReason::ModifiedFeature("closures".into()))
        );
    }

    #[test]
    fn no_changes_means_failures_and_smoke_only() {
        let metadata = vec![
            TestMetadata::new("failed"),
            TestMetadata::new("smoke").smoke(),
            TestMetadata::new("ordinary"),
        ];
        let selection = select(
            metadata,
            &ChangeSet::new(),
            &FailureState::from_failing(["failed"]),
        );
        assert_eq!(ids(&selection), ["failed", "smoke"]);
    }

    #[test]
    fn full_run_selects_everything_with_stable_priority() {
        let metadata = vec![
            TestMetadata::new("ordinary"),
            TestMetadata::new("smoke").smoke(),
            TestMetadata::new("failed"),
        ];
        let selection = select(
            metadata,
            &ChangeSet::full_run(),
            &FailureState::from_failing(["failed"]),
        );
        assert_eq!(ids(&selection), ["failed", "smoke", "ordinary"]);
        assert!(
            selection
                .tests
                .iter()
                .all(|test| test.has_reason(&SelectionReason::FullRun))
        );
    }

    #[test]
    fn duplicate_metadata_and_input_order_do_not_change_output() {
        let first = vec![
            TestMetadata::new("same").feature("one"),
            TestMetadata::new("same").host_op("write"),
            TestMetadata::new("alpha").smoke(),
        ];
        let second = vec![first[2].clone(), first[1].clone(), first[0].clone()];
        let changes = ChangeSet::new().host_op("write");
        let a = select(first, &changes, &FailureState::new());
        let b = select(second, &changes, &FailureState::new());
        assert_eq!(a, b);
        assert_eq!(ids(&a), ["alpha", "same"]);
    }

    #[test]
    fn failure_state_updates_and_deduplicates() {
        let mut state = FailureState::new();
        state.record_failure("a");
        state.record_failure("a");
        state.update_result("b", false);
        state.update_result("a", true);
        state.update_from_results([("b", true), ("c", false)]);
        assert_eq!(
            state
                .failing_tests()
                .map(TestId::as_str)
                .collect::<Vec<_>>(),
            ["c"]
        );
    }

    #[test]
    fn failure_state_persistence_is_sorted_and_round_trips_special_ids() {
        let state = FailureState::from_failing(["z/test.js", "a\tnewline\n\u{1f525}.js", "middle"]);
        let text = state.serialize();
        assert_eq!(
            text,
            "kiln-test-selector-failures-v1\nfailed\ta%09newline%0A%F0%9F%94%A5.js\nfailed\tmiddle\nfailed\tz/test.js\n"
        );
        assert_eq!(FailureState::parse(&text), Ok(state.clone()));
        assert_eq!(FailureState::from_text(&text), Ok(state));
    }

    #[test]
    fn malformed_persistence_is_rejected() {
        assert_eq!(
            FailureState::parse("wrong\n"),
            Err(PersistenceError::InvalidHeader)
        );
        assert_eq!(
            FailureState::parse("kiln-test-selector-failures-v1\nbad\n"),
            Err(PersistenceError::MalformedLine { line: 2 })
        );
        assert!(matches!(
            FailureState::parse("kiln-test-selector-failures-v1\nfailed\t%GG\n"),
            Err(PersistenceError::InvalidEscape {
                line: 2,
                kind: EscapeError::NonHex
            })
        ));
    }

    #[test]
    fn empty_and_trailing_newline_variants_parse() {
        let state = FailureState::new();
        assert_eq!(FailureState::parse(&state.serialize()), Ok(state.clone()));
        assert_eq!(
            FailureState::parse("kiln-test-selector-failures-v1"),
            Ok(state)
        );
    }
}
