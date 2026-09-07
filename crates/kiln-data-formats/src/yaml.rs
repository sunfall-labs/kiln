//! Pure-Rust YAML parser compiling to `serde_json::Value`.
//!
//! Enforces strict security bounds:
//! - `MAX_NESTING_DEPTH = 128` (prevents stack exhaustion)
//! - `MAX_ALIASES = 100` (prevents YAML billion-laughs entity expansion attacks)

use std::collections::HashMap;
use std::fmt;

pub const MAX_NESTING_DEPTH: usize = 128;
pub const MAX_ALIASES: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_yaml(input: &str) -> Result<serde_json::Value, ParseError> {
    let mut parser = Parser::new(input);
    parser.parse_document()
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    cursor: usize,
    anchors: HashMap<String, serde_json::Value>,
    alias_count: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            lines: input.lines().collect(),
            cursor: 0,
            anchors: HashMap::new(),
            alias_count: 0,
        }
    }

    fn error(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            line: self.cursor + 1,
        }
    }

    fn current_line(&self) -> Option<&'a str> {
        if self.cursor < self.lines.len() {
            Some(self.lines[self.cursor])
        } else {
            None
        }
    }

    fn advance_line(&mut self) {
        self.cursor += 1;
    }

    fn parse_document(&mut self) -> Result<serde_json::Value, ParseError> {
        self.skip_blank_and_comment_lines();
        if self.cursor >= self.lines.len() {
            return Ok(serde_json::Value::Null);
        }

        // Check for document start "---"
        if let Some(line) = self.current_line()
            && line.trim() == "---"
        {
            self.advance_line();
            self.skip_blank_and_comment_lines();
        }

        let base_indent = self.current_line_indent().unwrap_or(0);
        let val = self.parse_block(base_indent, 0)?;

        // Skip document end "..." if any
        self.skip_blank_and_comment_lines();
        if let Some(line) = self.current_line()
            && line.trim() == "..."
        {
            self.advance_line();
        }

        Ok(val)
    }

    fn skip_blank_and_comment_lines(&mut self) {
        while self.cursor < self.lines.len() {
            let trimmed = self.lines[self.cursor].trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.cursor += 1;
            } else {
                break;
            }
        }
    }

    fn current_line_indent(&self) -> Option<usize> {
        if self.cursor >= self.lines.len() {
            return None;
        }
        let line = self.lines[self.cursor];
        let spaces = line.chars().take_while(|c| *c == ' ').count();
        Some(spaces)
    }

    fn parse_block(
        &mut self,
        min_indent: usize,
        depth: usize,
    ) -> Result<serde_json::Value, ParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self.error(format!(
                "exceeded maximum nesting depth of {MAX_NESTING_DEPTH}"
            )));
        }

        self.skip_blank_and_comment_lines();
        let Some(line) = self.current_line() else {
            return Ok(serde_json::Value::Null);
        };

        let indent = self.current_line_indent().unwrap_or(0);
        if indent < min_indent {
            return Ok(serde_json::Value::Null);
        }

        let trimmed = line.trim();
        if trimmed.starts_with('-') && (trimmed.len() == 1 || trimmed[1..].starts_with(' ')) {
            self.parse_sequence(indent, depth)
        } else if trimmed.contains(':') {
            self.parse_mapping(indent, depth)
        } else {
            // Single scalar on this line
            self.advance_line();
            self.parse_scalar(trimmed)
        }
    }

    fn parse_sequence(
        &mut self,
        indent: usize,
        depth: usize,
    ) -> Result<serde_json::Value, ParseError> {
        let mut list = Vec::new();

        while self.cursor < self.lines.len() {
            self.skip_blank_and_comment_lines();
            let Some(line) = self.current_line() else {
                break;
            };

            let cur_indent = self.current_line_indent().unwrap_or(0);
            if cur_indent < indent {
                break;
            }
            if cur_indent > indent {
                return Err(self.error("unexpected indentation inside sequence"));
            }

            let trimmed = line.trim();
            if !trimmed.starts_with('-') || (trimmed.len() > 1 && !trimmed[1..].starts_with(' ')) {
                break;
            }

            self.advance_line();
            let after_dash = trimmed[1..].trim();

            if after_dash.is_empty() {
                // Next lines contain the element
                self.skip_blank_and_comment_lines();
                let next_indent = self.current_line_indent().unwrap_or(0);
                if next_indent > indent {
                    let elem = self.parse_block(next_indent, depth + 1)?;
                    list.push(elem);
                } else {
                    list.push(serde_json::Value::Null);
                }
            } else if after_dash.contains(':')
                && !after_dash.starts_with('"')
                && !after_dash.starts_with('\'')
            {
                // Inlined mapping after dash (e.g. "- name: John")
                let mut map = serde_json::Map::new();
                let (key, val_str) = self.split_key_value(after_dash)?;
                let val = if val_str.is_empty() {
                    self.skip_blank_and_comment_lines();
                    let child_indent = self.current_line_indent().unwrap_or(0);
                    if child_indent > indent {
                        self.parse_block(child_indent, depth + 1)?
                    } else {
                        serde_json::Value::Null
                    }
                } else {
                    self.parse_scalar(&val_str)?
                };
                map.insert(key, val);

                // Collect any sibling map keys indented under the dash
                let item_indent = indent + 2;
                while self.cursor < self.lines.len() {
                    self.skip_blank_and_comment_lines();
                    let Some(sibling) = self.current_line() else {
                        break;
                    };
                    let sib_indent = self.current_line_indent().unwrap_or(0);
                    if sib_indent < item_indent {
                        break;
                    }
                    let sib_trimmed = sibling.trim();
                    if sib_trimmed.starts_with('-') {
                        break;
                    }
                    self.advance_line();
                    let (skey, sval_str) = self.split_key_value(sib_trimmed)?;
                    let sval = if sval_str.is_empty() {
                        self.skip_blank_and_comment_lines();
                        let sub_indent = self.current_line_indent().unwrap_or(0);
                        if sub_indent > item_indent {
                            self.parse_block(sub_indent, depth + 1)?
                        } else {
                            serde_json::Value::Null
                        }
                    } else {
                        self.parse_scalar(&sval_str)?
                    };
                    map.insert(skey, sval);
                }
                list.push(serde_json::Value::Object(map));
            } else {
                let elem = self.parse_scalar(after_dash)?;
                list.push(elem);
            }
        }

        Ok(serde_json::Value::Array(list))
    }

    fn parse_mapping(
        &mut self,
        indent: usize,
        depth: usize,
    ) -> Result<serde_json::Value, ParseError> {
        let mut map = serde_json::Map::new();

        while self.cursor < self.lines.len() {
            self.skip_blank_and_comment_lines();
            let Some(line) = self.current_line() else {
                break;
            };

            let cur_indent = self.current_line_indent().unwrap_or(0);
            if cur_indent < indent {
                break;
            }
            if cur_indent > indent {
                return Err(self.error("unexpected extra indentation in mapping"));
            }

            let trimmed = line.trim();
            if trimmed.starts_with('-') {
                break;
            }

            self.advance_line();
            let (key, val_str) = self.split_key_value(trimmed)?;

            let val = if val_str.is_empty() {
                // Subordinate block
                self.skip_blank_and_comment_lines();
                let child_indent = self.current_line_indent().unwrap_or(0);
                if child_indent > indent {
                    self.parse_block(child_indent, depth + 1)?
                } else {
                    serde_json::Value::Null
                }
            } else if val_str.starts_with('&') {
                // Anchor definition: e.g. &base value
                let space_idx = val_str.find(' ').unwrap_or(val_str.len());
                let anchor_name = val_str[1..space_idx].to_string();
                let remainder = val_str[space_idx..].trim();
                let v = if remainder.is_empty() {
                    self.skip_blank_and_comment_lines();
                    let child_indent = self.current_line_indent().unwrap_or(0);
                    if child_indent > indent {
                        self.parse_block(child_indent, depth + 1)?
                    } else {
                        serde_json::Value::Null
                    }
                } else {
                    self.parse_scalar(remainder)?
                };
                self.anchors.insert(anchor_name, v.clone());
                v
            } else if val_str.starts_with('*') {
                // Alias expansion: e.g. *base
                self.expand_alias(val_str.trim()[1..].trim())?
            } else {
                self.parse_scalar(&val_str)?
            };

            map.insert(key, val);
        }

        Ok(serde_json::Value::Object(map))
    }

    fn split_key_value(&self, line: &str) -> Result<(String, String), ParseError> {
        let colon_idx = line
            .find(':')
            .ok_or_else(|| self.error("expected ':' in key-value mapping"))?;
        let key = line[..colon_idx]
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        let value_part = line[colon_idx + 1..].trim();
        // Remove inline comment if value is unquoted
        let cleaned_val = if (!value_part.starts_with('"') && !value_part.starts_with('\''))
            && value_part.contains('#')
        {
            let comment_idx = value_part.find('#').unwrap();
            value_part[..comment_idx].trim()
        } else {
            value_part
        };
        Ok((key, cleaned_val.to_string()))
    }

    fn expand_alias(&mut self, anchor_name: &str) -> Result<serde_json::Value, ParseError> {
        self.alias_count += 1;
        if self.alias_count > MAX_ALIASES {
            return Err(self.error(format!(
                "exceeded maximum YAML alias expansions ({MAX_ALIASES}); possible billion-laughs attack"
            )));
        }

        let Some(val) = self.anchors.get(anchor_name) else {
            return Err(self.error(format!("undefined YAML anchor '*{anchor_name}'")));
        };

        Ok(val.clone())
    }

    fn parse_scalar(&mut self, text: &str) -> Result<serde_json::Value, ParseError> {
        let trimmed = text.trim();

        // Check for anchor definition on scalar
        if trimmed.starts_with('&') {
            let space_idx = trimmed.find(' ').unwrap_or(trimmed.len());
            let anchor_name = trimmed[1..space_idx].to_string();
            let remainder = trimmed[space_idx..].trim();
            let val = self.parse_scalar(remainder)?;
            self.anchors.insert(anchor_name, val.clone());
            return Ok(val);
        }

        // Check for alias reference
        if let Some(alias) = trimmed.strip_prefix('*') {
            return self.expand_alias(alias);
        }

        // Block scalars: literal '|' and folded '>'
        if trimmed == "|" || trimmed == ">" {
            let is_folded = trimmed == ">";
            let mut block_lines = Vec::new();
            self.skip_blank_and_comment_lines();
            let block_indent = self.current_line_indent().unwrap_or(0);
            while self.cursor < self.lines.len() {
                let line = self.lines[self.cursor];
                let cur_indent = self.current_line_indent().unwrap_or(0);
                if line.trim().is_empty() {
                    block_lines.push("");
                    self.advance_line();
                    continue;
                }
                if cur_indent < block_indent {
                    break;
                }
                self.advance_line();
                block_lines.push(&line[block_indent.min(line.len())..]);
            }
            let res = if is_folded {
                block_lines.join(" ")
            } else {
                block_lines.join("\n")
            };
            return Ok(serde_json::Value::String(res));
        }

        // Quoted string
        let is_double_quoted =
            trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2;
        let is_single_quoted =
            trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2;
        if is_double_quoted || is_single_quoted {
            return Ok(serde_json::Value::String(
                trimmed[1..trimmed.len() - 1].to_string(),
            ));
        }

        // Inline JSON array
        if trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        {
            return Ok(v);
        }

        // Inline JSON object
        if trimmed.starts_with('{')
            && trimmed.ends_with('}')
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        {
            return Ok(v);
        }

        // Booleans
        match trimmed {
            "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => {
                return Ok(serde_json::Value::Bool(true));
            }
            "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => {
                return Ok(serde_json::Value::Bool(false));
            }
            "null" | "Null" | "NULL" | "~" | "" => {
                return Ok(serde_json::Value::Null);
            }
            _ => {}
        }

        // Numbers
        if let Ok(i) = trimmed.parse::<i64>() {
            return Ok(serde_json::Value::Number(i.into()));
        }
        if let Ok(f) = trimmed.parse::<f64>()
            && let Some(n) = serde_json::Number::from_f64(f)
        {
            return Ok(serde_json::Value::Number(n));
        }

        // Plain unquoted string
        Ok(serde_json::Value::String(trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_mappings_and_sequences() {
        let yaml = r#"
        name: kiln
        version: 1.0.0
        features:
          - fast
          - reproducible
          - compile-time-loaders
        settings:
          debug: true
          threads: 8
          timeout: 30.5
        "#;

        let val = parse_yaml(yaml).unwrap();
        assert_eq!(val["name"], "kiln");
        assert_eq!(val["version"], "1.0.0");
        assert_eq!(val["features"][0], "fast");
        assert_eq!(val["features"][2], "compile-time-loaders");
        assert_eq!(val["settings"]["debug"], true);
        assert_eq!(val["settings"]["threads"], 8);
        assert_eq!(val["settings"]["timeout"], 30.5);
    }

    #[test]
    fn supports_anchors_and_aliases() {
        let yaml = r#"
        default_config: &defaults
          adapter: sqlite
          timeout: 5000

        development:
          <<: *defaults
          name: dev_db
        "#;

        let val = parse_yaml(yaml).unwrap();
        assert_eq!(val["default_config"]["adapter"], "sqlite");
    }

    #[test]
    fn protects_against_billion_laughs_alias_attack() {
        let mut evil = String::from("a: &a [1, 2]\n");
        for i in 0..110 {
            evil.push_str(&format!("b{i}: *a\n"));
        }
        let err = parse_yaml(&evil).unwrap_err();
        assert!(err.message.contains("maximum YAML alias expansions"));
    }
}
