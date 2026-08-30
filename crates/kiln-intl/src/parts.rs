//! Reading ICU4X's own run annotations back out of a formatted string.
//!
//! `formatToParts` has to say where each run of a formatted string begins and ends and what it is:
//! which characters are the integer digits, which are the group separator, which are a list
//! element and which are the text between elements. Recovering that from the finished string by
//! searching for separators works until a locale's separator also appears inside an element, which
//! is a wrong answer rather than a missing one.
//!
//! ICU4X already knows -- it emits [`writeable::Part`] annotations as it writes -- so this is a
//! sink that records them instead of a parser that guesses them.

/// Collects the runs of a `Writeable`, naming each one with the caller's vocabulary.
pub(crate) struct Collector {
    parts: Vec<(&'static str, String)>,
    current: Option<&'static str>,
    name: fn(writeable::Part) -> &'static str,
}

impl Collector {
    /// A collector that names ICU4X's parts with `name`.
    pub(crate) fn new(name: fn(writeable::Part) -> &'static str) -> Self {
        Self {
            parts: Vec::new(),
            current: None,
            name,
        }
    }

    /// The runs, in order, with adjacent runs of the same kind merged.
    ///
    /// Merging matters: ICU4X writes an integer's digits in group-sized chunks, and the language
    /// wants one `integer` part per run between separators rather than one per chunk.
    pub(crate) fn finish(self) -> Vec<(&'static str, String)> {
        self.parts
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Text written outside any annotation is the specification's `literal`.
        let name = self.current.unwrap_or("literal");
        match self.parts.last_mut() {
            Some((last, buffer)) if *last == name => buffer.push_str(text),
            _ => self.parts.push((name, text.to_string())),
        }
    }
}

impl core::fmt::Write for Collector {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.push(text);
        Ok(())
    }
}

impl writeable::PartsWrite for Collector {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: writeable::Part,
        mut f: impl FnMut(&mut Self::SubPartsWrite) -> core::fmt::Result,
    ) -> core::fmt::Result {
        let previous = self.current;
        self.current = Some((self.name)(part));
        let result = f(self);
        self.current = previous;
        result
    }
}
