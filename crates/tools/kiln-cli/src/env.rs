//! Automatic `.env` loading with variable expansion for Kiln projects.
//!
//! Follows standard 12-factor and modern Node/Vite conventions:
//! - Precedence order: process env > `.env.[mode].local` > `.env.local` > `.env.[mode]` > `.env`
//! - `.env.local` is skipped during test runs (`mode == "test"`).
//! - Variables already set in `std::env` are never overwritten.
//! - Variable expansion supports `${VAR}` and `${VAR:-default}`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Load environment files for `project_root` into `std::env`.
/// Returns the list of newly set environment variables (key, value).
pub fn load_project_env(project_root: &Path, is_test: bool) -> Vec<(String, String)> {
    let mode = if is_test {
        "test".to_string()
    } else {
        std::env::var("NODE_ENV").unwrap_or_else(|_| "development".to_string())
    };

    let mut candidate_files = Vec::new();
    if !mode.is_empty() {
        candidate_files.push(format!(".env.{mode}.local"));
    }
    if !is_test {
        candidate_files.push(".env.local".to_string());
    }
    if !mode.is_empty() {
        candidate_files.push(format!(".env.{mode}"));
    }
    candidate_files.push(".env".to_string());

    let mut merged: HashMap<String, String> = HashMap::new();

    for filename in candidate_files {
        let path = project_root.join(&filename);
        if let Ok(content) = fs::read_to_string(&path) {
            let parsed = parse_env(&content);
            for (key, raw_val) in parsed {
                // Higher-precedence files set the key first; do not overwrite.
                merged.entry(key).or_insert(raw_val);
            }
        }
    }

    let mut newly_set = Vec::new();
    for (key, raw_val) in merged {
        // Process environment always wins.
        if std::env::var_os(&key).is_none() {
            let expanded = expand_value(&raw_val, &|name| std::env::var(name).ok());
            // SAFETY: In single-threaded CLI setup phase before spawning worker threads.
            unsafe {
                std::env::set_var(&key, &expanded);
            }
            newly_set.push((key, expanded));
        }
    }

    newly_set
}

/// Parse lines of a `.env` file into key-value pairs without expanding variables yet.
pub fn parse_env(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Allow optional `export ` prefix
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();

        let Some(equals_pos) = trimmed.find('=') else {
            continue;
        };

        let key = trimmed[..equals_pos].trim().to_string();
        if key.is_empty() {
            continue;
        }

        let raw_val = trimmed[equals_pos + 1..].trim();
        let val = parse_raw_value(raw_val);
        pairs.push((key, val));
    }

    pairs
}

fn parse_raw_value(val: &str) -> String {
    if val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2 {
        // Single quotes: literal value, no escapes or variable expansions
        val[1..val.len() - 1].to_string()
    } else if val.starts_with('"') && val.ends_with('"') && val.len() >= 2 {
        // Double quotes: unescape standard sequences
        unescape_double_quotes(&val[1..val.len() - 1])
    } else {
        // Unquoted: strip inline comments (e.g. `value # comment`)
        let without_comment = if let Some(idx) = val.find('#') {
            let before = &val[..idx];
            if before.ends_with(|c: char| c.is_whitespace()) {
                before.trim_end()
            } else {
                val
            }
        } else {
            val
        };
        without_comment.to_string()
    }
}

fn unescape_double_quotes(val: &str) -> String {
    let mut result = String::with_capacity(val.len());
    let mut chars = val.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('$') => result.push('$'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Expand `${VAR}` and `${VAR:-default}` patterns.
pub fn expand_value(val: &str, lookup: &impl Fn(&str) -> Option<String>) -> String {
    let mut result = String::with_capacity(val.len());
    let mut chars = val.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_expr = String::new();
            let mut closed = false;

            for ch in chars.by_ref() {
                if ch == '}' {
                    closed = true;
                    break;
                }
                var_expr.push(ch);
            }

            if closed {
                if let Some(idx) = var_expr.find(":-") {
                    let var_name = &var_expr[..idx];
                    let default_val = &var_expr[idx + 2..];
                    match lookup(var_name) {
                        Some(val) if !val.is_empty() => result.push_str(&val),
                        _ => result.push_str(default_val),
                    }
                } else if let Some(val) = lookup(&var_expr) {
                    result.push_str(&val);
                }
            } else {
                result.push_str("${");
                result.push_str(&var_expr);
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_and_quoted_values() {
        let env = r#"
        # Comment line
        FOO=bar
        export BAZ="hello world\n"
        LITERAL='not_${expanded}'
        UNQUOTED=some_value # inline comment
        EMPTY=
        "#;
        let pairs = parse_env(env);
        assert_eq!(
            pairs,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "hello world\n".to_string()),
                ("LITERAL".to_string(), "not_${expanded}".to_string()),
                ("UNQUOTED".to_string(), "some_value".to_string()),
                ("EMPTY".to_string(), "".to_string()),
            ]
        );
    }

    #[test]
    fn expands_variables_with_fallbacks() {
        let env_map: HashMap<&str, &str> = [("HOST", "localhost"), ("PORT", "8080")]
            .into_iter()
            .collect();
        let lookup = |name: &str| env_map.get(name).map(|s| s.to_string());

        assert_eq!(
            expand_value("http://${HOST}:${PORT}/api", &lookup),
            "http://localhost:8080/api"
        );
        assert_eq!(
            expand_value("${MISSING:-default_val}", &lookup),
            "default_val"
        );
        assert_eq!(expand_value("${HOST:-default_val}", &lookup), "localhost");
        assert_eq!(expand_value("no_braces_$HOST", &lookup), "no_braces_$HOST");
    }

    #[test]
    fn loads_and_merges_env_files_in_precedence() {
        let temp = std::env::temp_dir().join(format!("kiln-env-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        fs::write(temp.join(".env"), "BASE=1\nSHARED=base\n").unwrap();
        fs::write(temp.join(".env.local"), "LOCAL=1\nSHARED=local\n").unwrap();
        fs::write(temp.join(".env.development"), "DEV=1\nSHARED=dev\n").unwrap();

        // Ensure process env is clear for our keys
        let keys = ["BASE", "LOCAL", "DEV", "SHARED"];
        for k in &keys {
            unsafe {
                std::env::remove_var(k);
            }
        }

        let newly_set = load_project_env(&temp, false);
        let set_map: HashMap<String, String> = newly_set.into_iter().collect();

        assert_eq!(set_map.get("BASE").map(String::as_str), Some("1"));
        assert_eq!(set_map.get("DEV").map(String::as_str), Some("1"));
        assert_eq!(set_map.get("LOCAL").map(String::as_str), Some("1"));
        // .env.development.local > .env.local > .env.development > .env
        assert_eq!(set_map.get("SHARED").map(String::as_str), Some("local"));

        for k in &keys {
            unsafe {
                std::env::remove_var(k);
            }
        }
        let _ = fs::remove_dir_all(&temp);
    }
}
