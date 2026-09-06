//! Lowering of static regular expression literals to direct C matchers.
//!
//! ECMAScript regular expressions defined as literals (e.g. `/.../flags`) with patterns known
//! at compile time can be lowered to optimized C state machines and character class matchers,
//! avoiding runtime engine interpretation and allocations. Complex constructs (such as backreferences
//! and lookarounds) fall back cleanly to `regress`.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegexFlags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub sticky: bool,
    pub unicode: bool,
}

impl RegexFlags {
    pub fn parse(flags_str: &str) -> Option<Self> {
        let mut flags = RegexFlags::default();
        let mut seen = [false; 256];
        for b in flags_str.bytes() {
            if seen[b as usize] {
                return None; // Duplicate flag rejected in JS
            }
            seen[b as usize] = true;
            match b {
                b'g' => flags.global = true,
                b'i' => flags.ignore_case = true,
                b'm' => flags.multiline = true,
                b's' => flags.dot_all = true,
                b'y' => flags.sticky = true,
                b'u' | b'v' => flags.unicode = true,
                b'd' => {} // HasIndices flag
                _ => return None,
            }
        }
        Some(flags)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharClass {
    /// Disjoint sorted inclusive ranges [start, end].
    pub ranges: Vec<(u16, u16)>,
    pub negated: bool,
    pub is_dot: bool,
    /// `\s` is a member of the class: the union of `ranges` and the whitespace set. The
    /// negation applies to the whole union, so `[^\s:]` is neither whitespace nor a colon.
    pub is_whitespace: bool,
    /// `\S` is a member: everything that is not whitespace.
    pub is_non_whitespace: bool,
}

impl CharClass {
    pub fn single(c: u16, ignore_case: bool) -> Self {
        if ignore_case && (c as u8).is_ascii_lowercase() {
            let lower = c;
            let upper = c - 32;
            Self {
                ranges: vec![(upper, upper), (lower, lower)],
                negated: false,
                is_dot: false,
                is_whitespace: false,
                is_non_whitespace: false,
            }
        } else if ignore_case && (c as u8).is_ascii_uppercase() {
            let upper = c;
            let lower = c + 32;
            Self {
                ranges: vec![(upper, upper), (lower, lower)],
                negated: false,
                is_dot: false,
                is_whitespace: false,
                is_non_whitespace: false,
            }
        } else {
            Self {
                ranges: vec![(c, c)],
                negated: false,
                is_dot: false,
                is_whitespace: false,
                is_non_whitespace: false,
            }
        }
    }

    pub fn dot() -> Self {
        Self {
            ranges: Vec::new(),
            negated: false,
            is_dot: true,
            is_whitespace: false,
            is_non_whitespace: false,
        }
    }

    pub fn whitespace(negated: bool) -> Self {
        Self {
            ranges: Vec::new(),
            negated,
            is_dot: false,
            is_whitespace: true,
            is_non_whitespace: false,
        }
    }

    pub fn digits(negated: bool) -> Self {
        Self {
            ranges: vec![(b'0' as u16, b'9' as u16)],
            negated,
            is_dot: false,
            is_whitespace: false,
            is_non_whitespace: false,
        }
    }

    pub fn word(negated: bool) -> Self {
        Self {
            ranges: vec![
                (b'0' as u16, b'9' as u16),
                (b'A' as u16, b'Z' as u16),
                (b'_' as u16, b'_' as u16),
                (b'a' as u16, b'z' as u16),
            ],
            negated,
            is_dot: false,
            is_whitespace: false,
            is_non_whitespace: false,
        }
    }

    pub fn matches(&self, c: u16, dot_all: bool) -> bool {
        if self.is_dot {
            if dot_all {
                return true;
            }
            return c != 0x000A && c != 0x000D && c != 0x2028 && c != 0x2029;
        }
        let in_range = self
            .ranges
            .iter()
            .any(|&(start, end)| c >= start && c <= end)
            || (self.is_whitespace && is_ecma_whitespace(c))
            || (self.is_non_whitespace && !is_ecma_whitespace(c));
        if self.negated { !in_range } else { in_range }
    }
}

pub fn is_ecma_whitespace(c: u16) -> bool {
    c == 0x0020
        || (0x0009..=0x000D).contains(&c)
        || c == 0x00A0
        || c == 0xFEFF
        || (0x2000..=0x200A).contains(&c)
        || c == 0x2028
        || c == 0x2029
        || c == 0x202F
        || c == 0x205F
        || c == 0x3000
        || c == 0x1680
}

pub fn is_ecma_line_terminator(c: u16) -> bool {
    c == 0x000A || c == 0x000D || c == 0x2028 || c == 0x2029
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegexTerm {
    AnchorStart,
    AnchorEnd,
    CharClass(usize),
    RepeatCharClass {
        class_index: usize,
        min: usize,
        max: Option<usize>,
        greedy: bool,
    },
    GroupStart(usize),
    GroupEnd(usize),
    Group {
        group_index: usize,
        terms: Vec<RegexTerm>,
    },
    OptionalGroup {
        group_index: usize,
        terms: Vec<RegexTerm>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub terms: Vec<RegexTerm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRegex {
    pub source: String,
    pub flags: RegexFlags,
    pub classes: Vec<CharClass>,
    pub branches: Vec<Branch>,
    pub group_count: usize,
    pub anchored_start: bool,
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    flags: RegexFlags,
    classes: Vec<CharClass>,
    group_counter: usize,
}

impl<'a> Parser<'a> {
    fn new(chars: &'a [char], flags: RegexFlags) -> Self {
        Self {
            chars,
            pos: 0,
            flags,
            classes: Vec::new(),
            group_counter: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn add_class(&mut self, class: CharClass) -> usize {
        for (i, existing) in self.classes.iter().enumerate() {
            if existing == &class {
                return i;
            }
        }
        let index = self.classes.len();
        self.classes.push(class);
        index
    }

    fn parse_disjunction(&mut self) -> Option<Vec<Branch>> {
        let mut branches = Vec::new();
        let first = self.parse_sequence()?;
        branches.push(Branch { terms: first });
        while self.peek() == Some('|') {
            self.next();
            let next_branch = self.parse_sequence()?;
            branches.push(Branch { terms: next_branch });
        }
        Some(branches)
    }

    fn parse_sequence(&mut self) -> Option<Vec<RegexTerm>> {
        let mut terms = Vec::new();
        while let Some(ch) = self.peek() {
            if ch == ')' || ch == '|' {
                break;
            }
            let term = self.parse_term()?;
            terms.push(term);
        }
        Some(terms)
    }

    fn parse_term(&mut self) -> Option<RegexTerm> {
        let ch = self.peek()?;
        match ch {
            '^' => {
                self.next();
                Some(RegexTerm::AnchorStart)
            }
            '$' => {
                self.next();
                Some(RegexTerm::AnchorEnd)
            }
            '(' => {
                self.next();
                if self.peek() == Some('?') {
                    self.next();
                    let marker = self.next()?;
                    if marker != ':' {
                        // Lookaround (?=, ?!, ?<=, ?<!), named group (?<), or mode (?flags) -> fallback
                        return None;
                    }
                    // Non-capturing group (?:...)
                    let inner = self.parse_sequence()?;
                    if self.next() != Some(')') {
                        return None;
                    }
                    // Non-capturing group terms directly or flat:
                    // If followed by quantifier:
                    if let Some((min, max, greedy)) = self.parse_quantifier() {
                        if inner.len() == 1 {
                            match &inner[0] {
                                RegexTerm::CharClass(class_idx) => {
                                    return Some(RegexTerm::RepeatCharClass {
                                        class_index: *class_idx,
                                        min,
                                        max,
                                        greedy,
                                    });
                                }
                                _ => return None,
                            }
                        } else {
                            return None; // Complex quantified non-capturing group fallback
                        }
                    }
                    // If no quantifier and single term, return it:
                    if inner.len() == 1 {
                        return Some(inner[0].clone());
                    }
                    // Otherwise return None for complex nested non-capturing groups
                    return None;
                }

                // Capturing group (...)
                self.group_counter += 1;
                let group_idx = self.group_counter;
                let inner = self.parse_sequence()?;
                if self.next() != Some(')') {
                    return None;
                }

                // Check for quantifier following group:
                if let Some((min, max, greedy)) = self.parse_quantifier() {
                    // Supported: optional group (a)? or (class)? where min=0, max=Some(1) with 1 char class
                    if min == 0
                        && max == Some(1)
                        && greedy
                        && inner.len() == 1
                        && matches!(inner[0], RegexTerm::CharClass(_))
                    {
                        return Some(RegexTerm::OptionalGroup {
                            group_index: group_idx,
                            terms: inner,
                        });
                    }
                    // Complex group quantifier: fallback to regress
                    return None;
                }

                // Normal capturing group
                Some(RegexTerm::Group {
                    group_index: group_idx,
                    terms: inner,
                })
            }
            '[' => {
                let class = self.parse_bracket_class()?;
                let class_idx = self.add_class(class);
                if let Some((min, max, greedy)) = self.parse_quantifier() {
                    Some(RegexTerm::RepeatCharClass {
                        class_index: class_idx,
                        min,
                        max,
                        greedy,
                    })
                } else {
                    Some(RegexTerm::CharClass(class_idx))
                }
            }
            '.' => {
                self.next();
                let class_idx = self.add_class(CharClass::dot());
                if let Some((min, max, greedy)) = self.parse_quantifier() {
                    Some(RegexTerm::RepeatCharClass {
                        class_index: class_idx,
                        min,
                        max,
                        greedy,
                    })
                } else {
                    Some(RegexTerm::CharClass(class_idx))
                }
            }
            '\\' => {
                self.next();
                let esc = self.next()?;
                let class = match esc {
                    'd' => CharClass::digits(false),
                    'D' => CharClass::digits(true),
                    's' => CharClass::whitespace(false),
                    'S' => CharClass::whitespace(true),
                    'w' => CharClass::word(false),
                    'W' => CharClass::word(true),
                    'b' | 'B' => return None, // Word boundaries -> fallback
                    '1'..='9' => return None, // Backreferences -> fallback
                    't' => CharClass::single(0x0009, self.flags.ignore_case),
                    'n' => CharClass::single(0x000A, self.flags.ignore_case),
                    'r' => CharClass::single(0x000D, self.flags.ignore_case),
                    'f' => CharClass::single(0x000C, self.flags.ignore_case),
                    'v' => CharClass::single(0x000B, self.flags.ignore_case),
                    '0' => CharClass::single(0x0000, self.flags.ignore_case),
                    'x' => {
                        let h1 = self.next()?.to_digit(16)? as u16;
                        let h2 = self.next()?.to_digit(16)? as u16;
                        CharClass::single((h1 << 4) | h2, self.flags.ignore_case)
                    }
                    'u' => {
                        if self.peek() == Some('{') {
                            self.next();
                            let mut val: u32 = 0;
                            while let Some(c) = self.peek() {
                                if c == '}' {
                                    self.next();
                                    break;
                                }
                                val = (val << 4) | (self.next()?.to_digit(16)?);
                            }
                            if val > 0xFFFF {
                                return None; // Surrogate pairs fallback
                            }
                            CharClass::single(val as u16, self.flags.ignore_case)
                        } else {
                            let mut val: u16 = 0;
                            for _ in 0..4 {
                                val = (val << 4) | (self.next()?.to_digit(16)? as u16);
                            }
                            CharClass::single(val, self.flags.ignore_case)
                        }
                    }
                    p => CharClass::single(p as u16, self.flags.ignore_case),
                };
                let class_idx = self.add_class(class);
                if let Some((min, max, greedy)) = self.parse_quantifier() {
                    Some(RegexTerm::RepeatCharClass {
                        class_index: class_idx,
                        min,
                        max,
                        greedy,
                    })
                } else {
                    Some(RegexTerm::CharClass(class_idx))
                }
            }
            c => {
                self.next();
                let class = CharClass::single(c as u16, self.flags.ignore_case);
                let class_idx = self.add_class(class);
                if let Some((min, max, greedy)) = self.parse_quantifier() {
                    Some(RegexTerm::RepeatCharClass {
                        class_index: class_idx,
                        min,
                        max,
                        greedy,
                    })
                } else {
                    Some(RegexTerm::CharClass(class_idx))
                }
            }
        }
    }

    fn parse_bracket_class(&mut self) -> Option<CharClass> {
        self.next(); // consume '['
        let negated = if self.peek() == Some('^') {
            self.next();
            true
        } else {
            false
        };

        let mut ranges: Vec<(u16, u16)> = Vec::new();
        let mut is_whitespace = false;
        let mut is_non_whitespace = false;

        while let Some(ch) = self.peek() {
            if ch == ']' {
                self.next();
                break;
            }
            let start_char = match ch {
                '\\' => {
                    self.next();
                    let esc = self.next()?;
                    match esc {
                        'd' => {
                            ranges.push((b'0' as u16, b'9' as u16));
                            continue;
                        }
                        'D' => {
                            return None;
                        }
                        's' => {
                            is_whitespace = true;
                            continue;
                        }
                        'S' => {
                            is_non_whitespace = true;
                            continue;
                        }
                        'w' => {
                            ranges.push((b'0' as u16, b'9' as u16));
                            ranges.push((b'A' as u16, b'Z' as u16));
                            ranges.push((b'_' as u16, b'_' as u16));
                            ranges.push((b'a' as u16, b'z' as u16));
                            continue;
                        }
                        'W' => return None,
                        't' => 0x0009,
                        'n' => 0x000A,
                        'r' => 0x000D,
                        'f' => 0x000C,
                        'v' => 0x000B,
                        '0' => 0x0000,
                        'x' => {
                            let h1 = self.next()?.to_digit(16)? as u16;
                            let h2 = self.next()?.to_digit(16)? as u16;
                            (h1 << 4) | h2
                        }
                        'u' => {
                            let mut val: u16 = 0;
                            for _ in 0..4 {
                                val = (val << 4) | (self.next()?.to_digit(16)? as u16);
                            }
                            val
                        }
                        p => p as u16,
                    }
                }
                c => {
                    self.next();
                    c as u16
                }
            };

            // Check for range: 'start_char - end_char'
            if self.peek() == Some('-') && self.chars.get(self.pos + 1) != Some(&']') {
                self.next(); // consume '-'
                let end_char = match self.next()? {
                    '\\' => match self.next()? {
                        't' => 0x0009,
                        'n' => 0x000A,
                        'r' => 0x000D,
                        'f' => 0x000C,
                        'v' => 0x000B,
                        '0' => 0x0000,
                        'x' => {
                            let h1 = self.next()?.to_digit(16)? as u16;
                            let h2 = self.next()?.to_digit(16)? as u16;
                            (h1 << 4) | h2
                        }
                        'u' => {
                            let mut val: u16 = 0;
                            for _ in 0..4 {
                                val = (val << 4) | (self.next()?.to_digit(16)? as u16);
                            }
                            val
                        }
                        p => p as u16,
                    },
                    c => c as u16,
                };
                if start_char > end_char {
                    return None; // Invalid range
                }
                ranges.push((start_char, end_char));
            } else {
                ranges.push((start_char, start_char));
            }
        }

        // Apply case folding if ignore_case:
        if self.flags.ignore_case {
            let mut folded_ranges = Vec::new();
            for (s, e) in ranges {
                folded_ranges.push((s, e));
                // If intersects A..Z:
                let cap_start = s.max(b'A' as u16);
                let cap_end = e.min(b'Z' as u16);
                if cap_start <= cap_end {
                    folded_ranges.push((cap_start + 32, cap_end + 32));
                }
                // If intersects a..z:
                let low_start = s.max(b'a' as u16);
                let low_end = e.min(b'z' as u16);
                if low_start <= low_end {
                    folded_ranges.push((low_start - 32, low_end - 32));
                }
            }
            ranges = folded_ranges;
        }

        // Sort and merge ranges
        ranges.sort_unstable_by_key(|r| r.0);
        let mut merged: Vec<(u16, u16)> = Vec::new();
        for (s, e) in ranges {
            if let Some(last) = merged.last_mut()
                && s <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(e);
                continue;
            }
            merged.push((s, e));
        }

        Some(CharClass {
            ranges: merged,
            negated,
            is_dot: false,
            is_whitespace,
            is_non_whitespace,
        })
    }

    fn parse_quantifier(&mut self) -> Option<(usize, Option<usize>, bool)> {
        let ch = self.peek()?;
        let (min, max) = match ch {
            '*' => {
                self.next();
                (0, None)
            }
            '+' => {
                self.next();
                (1, None)
            }
            '?' => {
                self.next();
                (0, Some(1))
            }
            '{' => {
                let saved_pos = self.pos;
                self.next();
                let mut num_str = String::new();
                while let Some(d) = self.peek() {
                    if d.is_ascii_digit() {
                        num_str.push(d);
                        self.next();
                    } else {
                        break;
                    }
                }
                if num_str.is_empty() {
                    self.pos = saved_pos;
                    return None;
                }
                let min: usize = num_str.parse().ok()?;
                let max = if self.peek() == Some(',') {
                    self.next();
                    let mut max_str = String::new();
                    while let Some(d) = self.peek() {
                        if d.is_ascii_digit() {
                            max_str.push(d);
                            self.next();
                        } else {
                            break;
                        }
                    }
                    if max_str.is_empty() {
                        None
                    } else {
                        Some(max_str.parse().ok()?)
                    }
                } else {
                    Some(min)
                };
                if self.next() != Some('}') {
                    self.pos = saved_pos;
                    return None;
                }
                (min, max)
            }
            _ => return None,
        };

        let greedy = if self.peek() == Some('?') {
            self.next();
            false
        } else {
            true
        };

        Some((min, max, greedy))
    }
}

pub fn try_compile_regex(pattern: &str, flags_str: &str) -> Option<CompiledRegex> {
    let flags = RegexFlags::parse(flags_str)?;
    let chars: Vec<char> = pattern.chars().collect();
    let mut parser = Parser::new(&chars, flags.clone());
    let mut branches = parser.parse_disjunction()?;
    if parser.pos != chars.len() {
        return None;
    }

    // Flatten group terms into sequence of terms with GroupStart and GroupEnd
    for branch in &mut branches {
        let mut flattened = Vec::new();
        for term in &branch.terms {
            match term {
                RegexTerm::Group { group_index, terms } => {
                    flattened.push(RegexTerm::GroupStart(*group_index));
                    flattened.extend(terms.clone());
                    flattened.push(RegexTerm::GroupEnd(*group_index));
                }
                other => flattened.push(other.clone()),
            }
        }
        branch.terms = flattened;
    }

    for branch in &branches {
        for term in &branch.terms {
            if let RegexTerm::OptionalGroup { terms, .. } = term
                && (terms.len() != 1 || !matches!(terms[0], RegexTerm::CharClass(_)))
            {
                return None;
            }
        }
    }

    let group_count = parser.group_counter;
    let anchored_start = !flags.multiline
        && !branches.is_empty()
        && branches
            .iter()
            .all(|b| matches!(b.terms.first(), Some(RegexTerm::AnchorStart)));

    Some(CompiledRegex {
        source: pattern.to_string(),
        flags,
        classes: parser.classes,
        branches,
        group_count,
        anchored_start,
    })
}

/// Emits the C code for the lowered regex matcher state machine.
pub fn emit_regex_matcher(output: &mut String, prefix: &str, compiled: &CompiledRegex) {
    writeln!(output, "// Lowered regex matcher for /{}/", compiled.source).unwrap();

    // 1. Emit character class predicates
    for (i, class) in compiled.classes.iter().enumerate() {
        writeln!(
            output,
            "static inline int {prefix}_class_{i}(uint16_t c) {{"
        )
        .unwrap();
        if class.is_dot {
            if compiled.flags.dot_all {
                writeln!(output, "    return 1;").unwrap();
            } else {
                writeln!(
                    output,
                    "    return c != 0x000A && c != 0x000D && c != 0x2028 && c != 0x2029;"
                )
                .unwrap();
            }
        } else {
            // The union of the ranges and the escape sets, then the negation over all of it.
            const WHITESPACE: &str = "(c == ' ' || (c >= 0x0009 && c <= 0x000D) || c == 0x00A0 || c == 0xFEFF || (c >= 0x2000 && c <= 0x200A) || c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F || c == 0x3000 || c == 0x1680)";
            let mut conds = Vec::new();
            for &(s, e) in &class.ranges {
                if s == e {
                    conds.push(format!("c == 0x{s:04x}"));
                } else {
                    conds.push(format!("(c >= 0x{s:04x} && c <= 0x{e:04x})"));
                }
            }
            if class.is_whitespace {
                conds.push(WHITESPACE.to_owned());
            }
            if class.is_non_whitespace {
                conds.push(format!("!{WHITESPACE}"));
            }
            if conds.is_empty() {
                writeln!(output, "    return {};", if class.negated { 1 } else { 0 }).unwrap();
            } else {
                let expr = conds.join(" || ");
                if class.negated {
                    writeln!(output, "    return !({expr});").unwrap();
                } else {
                    writeln!(output, "    return {expr};").unwrap();
                }
            }
        }
        writeln!(output, "}}").unwrap();
    }

    // 2. Emit uint16_t matcher
    emit_matcher_impl(output, prefix, compiled, false);

    // 3. Emit uint8_t (ASCII) matcher
    emit_matcher_impl(output, prefix, compiled, true);
}

fn emit_matcher_impl(output: &mut String, prefix: &str, compiled: &CompiledRegex, is_ascii: bool) {
    let fn_prefix = if is_ascii {
        format!("{prefix}_ascii")
    } else {
        prefix.to_string()
    };
    let ctx_type = if is_ascii {
        format!("{prefix}_ascii_ctx")
    } else {
        format!("{prefix}_ctx")
    };
    let char_type = if is_ascii { "uint8_t" } else { "uint16_t" };
    let matcher_fn_name = if is_ascii {
        format!("{prefix}_ascii")
    } else {
        prefix.to_string()
    };

    // Context struct
    writeln!(
        output,
        "typedef struct {{
    const {char_type} *units;
    size_t count;
    int64_t *ranges;
    size_t capacity;
    size_t end_pos;
}} {ctx_type};"
    )
    .unwrap();

    // Emit step functions for each branch
    for (b_idx, branch) in compiled.branches.iter().enumerate() {
        let step_count = branch.terms.len();
        // Forward declarations of step functions
        for s_idx in 0..=step_count {
            writeln!(
                output,
                "static inline __attribute__((always_inline)) int {fn_prefix}_b{b_idx}_step_{s_idx}({ctx_type} *ctx, size_t pos);"
            )
            .unwrap();
        }

        // Final step (success)
        writeln!(
            output,
            "static inline __attribute__((always_inline)) int {fn_prefix}_b{b_idx}_step_{step_count}({ctx_type} *ctx, size_t pos) {{
    ctx->end_pos = pos;
    return 1;
}}"
        )
        .unwrap();

        // Intermediate steps in reverse order
        for s_idx in (0..step_count).rev() {
            let term = &branch.terms[s_idx];
            writeln!(
                output,
                "static inline __attribute__((always_inline)) int {fn_prefix}_b{b_idx}_step_{s_idx}({ctx_type} *ctx, size_t pos) {{"
            )
            .unwrap();
            match term {
                RegexTerm::AnchorStart => {
                    if compiled.flags.multiline {
                        writeln!(
                            output,
                            "    if (pos != 0 && (ctx->units[pos - 1] != 0x000A && ctx->units[pos - 1] != 0x000D && ctx->units[pos - 1] != 0x2028 && ctx->units[pos - 1] != 0x2029)) return 0;
    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                            s_idx + 1
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            output,
                            "    if (pos != 0) return 0;
    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                            s_idx + 1
                        )
                        .unwrap();
                    }
                }
                RegexTerm::AnchorEnd => {
                    if compiled.flags.multiline {
                        writeln!(
                            output,
                            "    if (pos != ctx->count && (ctx->units[pos] != 0x000A && ctx->units[pos] != 0x000D && ctx->units[pos] != 0x2028 && ctx->units[pos] != 0x2029)) return 0;
    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                            s_idx + 1
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            output,
                            "    if (pos != ctx->count) return 0;
    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                            s_idx + 1
                        )
                        .unwrap();
                    }
                }
                RegexTerm::CharClass(class_idx) => {
                    writeln!(
                        output,
                        "    if (pos < ctx->count && {prefix}_class_{class_idx}(ctx->units[pos])) {{
        return {fn_prefix}_b{b_idx}_step_{}(ctx, pos + 1);
    }}
    return 0;",
                        s_idx + 1
                    )
                    .unwrap();
                }
                RegexTerm::RepeatCharClass {
                    class_index,
                    min,
                    max,
                    greedy,
                } => {
                    let max_limit = match max {
                        Some(m) => format!(" && m < {m}"),
                        None => String::new(),
                    };
                    writeln!(
                        output,
                        "    size_t m = 0;
    while (pos + m < ctx->count{max_limit} && {prefix}_class_{class_index}(ctx->units[pos + m])) {{
        m++;
    }}
    if (m < {min}) return 0;"
                    )
                    .unwrap();
                    if let Some(m_val) = max
                        && *min == *m_val
                    {
                        writeln!(
                            output,
                            "    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos + {min});",
                            s_idx + 1
                        )
                        .unwrap();
                        writeln!(output, "}}").unwrap();
                        continue;
                    }
                    if *greedy {
                        writeln!(
                            output,
                            "    for (size_t k = m; k >= {min}; --k) {{
        if ({fn_prefix}_b{b_idx}_step_{}(ctx, pos + k)) return 1;
        if (k == 0) break;
    }}
    return 0;",
                            s_idx + 1
                        )
                        .unwrap();
                    } else {
                        writeln!(
                            output,
                            "    for (size_t k = {min}; k <= m; ++k) {{
        if ({fn_prefix}_b{b_idx}_step_{}(ctx, pos + k)) return 1;
    }}
    return 0;",
                            s_idx + 1
                        )
                        .unwrap();
                    }
                }
                RegexTerm::GroupStart(g) => {
                    writeln!(
                        output,
                        "    int64_t old_start = -1;
    if (2 * {g} < ctx->capacity) {{
        old_start = ctx->ranges[2 * {g}];
        ctx->ranges[2 * {g}] = (int64_t)pos;
    }}
    if ({fn_prefix}_b{b_idx}_step_{}(ctx, pos)) return 1;
    if (2 * {g} < ctx->capacity) {{
        ctx->ranges[2 * {g}] = old_start;
    }}
    return 0;",
                        s_idx + 1
                    )
                    .unwrap();
                }
                RegexTerm::GroupEnd(g) => {
                    writeln!(
                        output,
                        "    int64_t old_end = -1;
    if (2 * {g} + 1 < ctx->capacity) {{
        old_end = ctx->ranges[2 * {g} + 1];
        ctx->ranges[2 * {g} + 1] = (int64_t)pos;
    }}
    if ({fn_prefix}_b{b_idx}_step_{}(ctx, pos)) return 1;
    if (2 * {g} + 1 < ctx->capacity) {{
        ctx->ranges[2 * {g} + 1] = old_end;
    }}
    return 0;",
                        s_idx + 1
                    )
                    .unwrap();
                }
                RegexTerm::Group { .. } => unreachable!(),
                RegexTerm::OptionalGroup { group_index, terms } => {
                    let g = *group_index;
                    if terms.len() == 1 {
                        if let RegexTerm::CharClass(class_index) = &terms[0] {
                            writeln!(
                                output,
                                "    if (pos < ctx->count && {prefix}_class_{class_index}(ctx->units[pos])) {{
        int64_t old_start = -1;
        int64_t old_end = -1;
        if (2 * {g} + 1 < ctx->capacity) {{
            old_start = ctx->ranges[2 * {g}];
            old_end = ctx->ranges[2 * {g} + 1];
            ctx->ranges[2 * {g}] = (int64_t)pos;
            ctx->ranges[2 * {g} + 1] = (int64_t)(pos + 1);
        }}
        if ({fn_prefix}_b{b_idx}_step_{}(ctx, pos + 1)) return 1;
        if (2 * {g} + 1 < ctx->capacity) {{
            ctx->ranges[2 * {g}] = old_start;
            ctx->ranges[2 * {g} + 1] = old_end;
        }}
    }}
    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                                s_idx + 1,
                                s_idx + 1
                            )
                            .unwrap();
                        } else {
                            writeln!(
                                output,
                                "    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                                s_idx + 1
                            )
                            .unwrap();
                        }
                    } else {
                        writeln!(
                            output,
                            "    return {fn_prefix}_b{b_idx}_step_{}(ctx, pos);",
                            s_idx + 1
                        )
                        .unwrap();
                    }
                }
            }
            writeln!(output, "}}").unwrap();
        }
    }

    // Main matcher function
    let total_groups = compiled.group_count + 1;
    let (early_check, try_limit_expr) = if compiled.anchored_start {
        ("start != 0 || start > count", "0")
    } else {
        ("start > count", "count")
    };

    writeln!(
        output,
        "static int {matcher_fn_name}(const {char_type} *units, size_t count, size_t start,
                     int64_t *ranges, size_t capacity, size_t *groups,
                     int *matched) {{
    *matched = 0;
    *groups = {total_groups};
    if (capacity / 2 < *groups) {{
        *groups = capacity / 2;
    }}
    if ({early_check}) {{
        return KILN_C99_OK;
    }}
    {ctx_type} ctx;
    ctx.units = units;
    ctx.count = count;
    ctx.ranges = ranges;
    ctx.capacity = capacity;
    ctx.end_pos = 0;

    size_t try_start = start;
    size_t try_limit = {try_limit_expr};
    size_t clear_groups = capacity / 2;
    if (clear_groups > {total_groups}) {{
        clear_groups = {total_groups};
    }}

    for (size_t pos = try_start; pos <= try_limit && pos <= count; ++pos) {{
        for (size_t g = 0; g < clear_groups; ++g) {{
            ranges[2 * g] = -1;
            ranges[2 * g + 1] = -1;
        }}
        if (capacity >= 1) {{
            ranges[0] = (int64_t)pos;
        }}

        /* Evaluate each branch in order */"
    )
    .unwrap();

    for (b_idx, _) in compiled.branches.iter().enumerate() {
        writeln!(
            output,
            "        if ({fn_prefix}_b{b_idx}_step_0(&ctx, pos)) {{
            if (capacity >= 2) {{
                ranges[1] = (int64_t)ctx.end_pos;
            }}
            *matched = 1;
            return KILN_C99_OK;
        }}"
        )
        .unwrap();
    }

    if compiled.flags.sticky {
        writeln!(output, "        break;").unwrap();
    }
    writeln!(
        output,
        "    }}
    return KILN_C99_OK;
}}"
    )
    .unwrap();
}

impl CompiledRegex {
    pub fn exec_simulated(&self, units: &[u16], start: usize) -> Option<Vec<(i64, i64)>> {
        let count = units.len();
        if start > count {
            return None;
        }
        let try_limit = if self.anchored_start {
            if start == 0 {
                0
            } else {
                return None;
            }
        } else {
            count
        };

        for pos in start..=try_limit {
            for branch in &self.branches {
                let mut ranges = vec![(-1, -1); self.group_count + 1];
                ranges[0].0 = pos as i64;
                if self.match_step(branch, 0, units, pos, &mut ranges) {
                    return Some(ranges);
                }
            }
            if self.flags.sticky {
                break;
            }
        }
        None
    }

    fn match_step(
        &self,
        branch: &Branch,
        step_idx: usize,
        units: &[u16],
        pos: usize,
        ranges: &mut [(i64, i64)],
    ) -> bool {
        if step_idx == branch.terms.len() {
            ranges[0].1 = pos as i64;
            return true;
        }

        match &branch.terms[step_idx] {
            RegexTerm::AnchorStart => {
                let ok = if self.flags.multiline {
                    pos == 0 || (pos > 0 && is_ecma_line_terminator(units[pos - 1]))
                } else {
                    pos == 0
                };
                if ok {
                    self.match_step(branch, step_idx + 1, units, pos, ranges)
                } else {
                    false
                }
            }
            RegexTerm::AnchorEnd => {
                let ok = if self.flags.multiline {
                    pos == units.len() || (pos < units.len() && is_ecma_line_terminator(units[pos]))
                } else {
                    pos == units.len()
                };
                if ok {
                    self.match_step(branch, step_idx + 1, units, pos, ranges)
                } else {
                    false
                }
            }
            RegexTerm::CharClass(class_idx) => {
                if pos < units.len()
                    && self.classes[*class_idx].matches(units[pos], self.flags.dot_all)
                {
                    self.match_step(branch, step_idx + 1, units, pos + 1, ranges)
                } else {
                    false
                }
            }
            RegexTerm::RepeatCharClass {
                class_index,
                min,
                max,
                greedy,
            } => {
                let mut m = 0;
                while pos + m < units.len()
                    && max.is_none_or(|limit| m < limit)
                    && self.classes[*class_index].matches(units[pos + m], self.flags.dot_all)
                {
                    m += 1;
                }
                if m < *min {
                    return false;
                }
                if let Some(limit) = max
                    && *min == *limit
                {
                    return self.match_step(branch, step_idx + 1, units, pos + *min, ranges);
                }
                if *greedy {
                    for k in (*min..=m).rev() {
                        if self.match_step(branch, step_idx + 1, units, pos + k, ranges) {
                            return true;
                        }
                    }
                    false
                } else {
                    for k in *min..=m {
                        if self.match_step(branch, step_idx + 1, units, pos + k, ranges) {
                            return true;
                        }
                    }
                    false
                }
            }
            RegexTerm::GroupStart(g) => {
                let old_start = ranges[*g].0;
                ranges[*g].0 = pos as i64;
                if self.match_step(branch, step_idx + 1, units, pos, ranges) {
                    true
                } else {
                    ranges[*g].0 = old_start;
                    false
                }
            }
            RegexTerm::GroupEnd(g) => {
                let old_end = ranges[*g].1;
                ranges[*g].1 = pos as i64;
                if self.match_step(branch, step_idx + 1, units, pos, ranges) {
                    true
                } else {
                    ranges[*g].1 = old_end;
                    false
                }
            }
            RegexTerm::Group { .. } => unreachable!(),
            RegexTerm::OptionalGroup { group_index, terms } => {
                if terms.len() == 1
                    && let RegexTerm::CharClass(class_index) = &terms[0]
                {
                    if pos < units.len()
                        && self.classes[*class_index].matches(units[pos], self.flags.dot_all)
                    {
                        let old_start = ranges[*group_index].0;
                        let old_end = ranges[*group_index].1;
                        ranges[*group_index].0 = pos as i64;
                        ranges[*group_index].1 = (pos + 1) as i64;
                        if self.match_step(branch, step_idx + 1, units, pos + 1, ranges) {
                            return true;
                        }
                        ranges[*group_index].0 = old_start;
                        ranges[*group_index].1 = old_end;
                    }
                    return self.match_step(branch, step_idx + 1, units, pos, ranges);
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_benchmark_pattern() {
        let pattern = r"\[(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z)\]\s+([A-Z]+)\s+([a-zA-Z0-9_\-\.]+):\s+(.*)";
        let compiled = try_compile_regex(pattern, "").expect("compiles benchmark regex");
        assert_eq!(compiled.group_count, 4);
        assert!(!compiled.anchored_start);
    }

    #[test]
    fn parses_common_patterns() {
        assert!(try_compile_regex(r"a(b)?c", "").is_some());
        assert!(try_compile_regex(r"^\d{2,3}$", "").is_some());
        assert!(try_compile_regex(r"[^a-z]", "i").is_some());
        assert!(try_compile_regex(r"x*", "g").is_some());
        assert!(try_compile_regex(r"a|b", "").is_some());
    }

    #[test]
    fn falls_back_on_complex_constructs() {
        // Backreference \1
        assert!(try_compile_regex(r"(\w)\1", "").is_none());
        // Lookaround (?<=...)
        assert!(try_compile_regex(r"(?<=a)b", "").is_none());
        // Lookahead (?!)
        assert!(try_compile_regex(r"a(?!b)", "").is_none());
        // Word boundary \b
        assert!(try_compile_regex(r"\bword\b", "").is_none());
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn parity_benchmark_sample() {
        let pattern = r"\[(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z)\]\s+([A-Z]+)\s+([a-zA-Z0-9_\-\.]+):\s+(.*)";
        let sample = "[2026-09-03T14:46:22.123Z] INFO http-server: request handled status=200 duration=12ms path=/api/v1/metrics";
        let sample_u16: Vec<u16> = sample.encode_utf16().collect();

        let compiled = try_compile_regex(pattern, "").unwrap();
        let regress_re = regress::Regex::new(pattern).unwrap();

        let regress_match = regress_re.find_from_utf16(&sample_u16, 0).next().unwrap();
        let sim_match = compiled.exec_simulated(&sample_u16, 0).unwrap();

        assert_eq!(sim_match.len(), 5);
        for g in 0..5 {
            let reg_range = regress_match.group(g);
            let sim_range = sim_match[g];
            match reg_range {
                Some(r) => {
                    assert_eq!(sim_range.0, r.start as i64);
                    assert_eq!(sim_range.1, r.end as i64);
                }
                None => {
                    assert_eq!(sim_range, (-1, -1));
                }
            }
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn parity_various_patterns() {
        let test_cases = vec![
            ("a(b)?c", "", "abc"),
            ("a(b)?c", "", "ac"),
            ("a(b)?c", "", "abbc"),
            ("(a)?(b)", "", "b"),
            ("(a)?(b)", "", "ab"),
            ("^\\d{2,3}$", "", "12"),
            ("^\\d{2,3}$", "", "123"),
            ("^\\d{2,3}$", "", "1"),
            ("^\\d{2,3}$", "", "1234"),
            ("[^a-z]", "i", "A"),
            ("[^a-z]", "i", "1"),
            ("x*", "", "abc"),
            ("x*", "", "xx"),
            ("a|b", "", "a"),
            ("a|b", "", "b"),
            ("a|b", "", "c"),
        ];

        for (pat, flags, input) in test_cases {
            let compiled = try_compile_regex(pat, flags).unwrap();
            let mut reg_flags = regress::Flags::default();
            if flags.contains('i') {
                reg_flags.icase = true;
            }
            if flags.contains('m') {
                reg_flags.multiline = true;
            }
            if flags.contains('s') {
                reg_flags.dot_all = true;
            }
            if flags.contains('u') {
                reg_flags.unicode = true;
            }
            let regress_re = regress::Regex::with_flags(pat, reg_flags).unwrap();

            let input_u16: Vec<u16> = input.encode_utf16().collect();
            let reg_match = regress_re.find_from_utf16(&input_u16, 0).next();
            let sim_match = compiled.exec_simulated(&input_u16, 0);

            assert_eq!(
                reg_match.is_some(),
                sim_match.is_some(),
                "match mismatch for /{pat}/{flags} on {input}"
            );
            if let (Some(rm), Some(sm)) = (reg_match, sim_match) {
                for g in 0..=compiled.group_count {
                    let r = rm.group(g);
                    let s = sm[g];
                    match r {
                        Some(rg) => {
                            assert_eq!(
                                s.0, rg.start as i64,
                                "group {g} start mismatch on /{pat}/ with {input}"
                            );
                            assert_eq!(
                                s.1, rg.end as i64,
                                "group {g} end mismatch on /{pat}/ with {input}"
                            );
                        }
                        None => {
                            assert_eq!(
                                s,
                                (-1, -1),
                                "group {g} expected undefined for /{pat}/ with {input}"
                            );
                        }
                    }
                }
            }
        }
    }
}
