//! Pure-Rust compile-time data format loaders (TOML, YAML, JSON5, JSONC).
//!
//! Inspired by Nub's `nub-data-formats`, these parsers run at compile time with
//! bounded execution (nesting limits, anti-billion-laughs alias bounds) to convert
//! structured configuration formats directly into serialized JSON representations
//! consumable by Kiln's interpreter and native backends.

pub mod json5;
pub mod toml;
pub mod yaml;

use std::fmt;
use std::path::Path;

pub const DATA_EXTENSIONS: &[&str] = &["json", "json5", "jsonc", "toml", "yaml", "yml"];

#[derive(Debug, Clone, PartialEq)]
pub enum DataFormatError {
    UnsupportedFormat(String),
    Json(String),
    Json5(json5::ParseError),
    Toml(toml::ParseError),
    Yaml(yaml::ParseError),
}

impl fmt::Display for DataFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat(ext) => write!(f, "unsupported data format extension: {ext}"),
            Self::Json(msg) => write!(f, "JSON parse error: {msg}"),
            Self::Json5(err) => write!(f, "JSON5 parse error: {err}"),
            Self::Toml(err) => write!(f, "TOML parse error: {err}"),
            Self::Yaml(err) => write!(f, "YAML parse error: {err}"),
        }
    }
}

impl std::error::Error for DataFormatError {}

/// Check whether an extension represents a supported compile-time data format.
pub fn is_data_extension(ext: &str) -> bool {
    DATA_EXTENSIONS.contains(&ext)
}

/// Parse content of a data file according to its path extension and convert it to JSON string.
pub fn parse_data_to_json(path: &Path, content: &str) -> Result<String, DataFormatError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let json_val = match ext.as_str() {
        "json" => {
            // Validate JSON syntax
            serde_json::from_str::<serde_json::Value>(content)
                .map_err(|e| DataFormatError::Json(e.to_string()))?
        }
        "json5" | "jsonc" => json5::parse_json5(content).map_err(DataFormatError::Json5)?,
        "toml" => toml::parse_toml(content).map_err(DataFormatError::Toml)?,
        "yaml" | "yml" => yaml::parse_yaml(content).map_err(DataFormatError::Yaml)?,
        _ => return Err(DataFormatError::UnsupportedFormat(ext)),
    };

    serde_json::to_string(&json_val).map_err(|e| DataFormatError::Json(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_to_json() {
        let path = Path::new("config.yaml");
        let content = "database:\n  host: localhost\n  port: 5432\n";
        let json_str = parse_data_to_json(path, content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["database"]["host"], "localhost");
        assert_eq!(parsed["database"]["port"], 5432);
    }

    #[test]
    fn parses_toml_to_json() {
        let path = Path::new("config.toml");
        let content = "[server]\nhost = \"0.0.0.0\"\nport = 8080\n";
        let json_str = parse_data_to_json(path, content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["server"]["host"], "0.0.0.0");
        assert_eq!(parsed["server"]["port"], 8080);
    }

    #[test]
    fn parses_json5_to_json() {
        let path = Path::new("settings.json5");
        let content = "{ // comment\n unquotedKey: 'value',\n }";
        let json_str = parse_data_to_json(path, content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["unquotedKey"], "value");
    }
}
