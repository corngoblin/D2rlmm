//! D2R's JSON files, which are not quite JSON.
//!
//! The game's UI layouts and localization tables contain comments, unquoted keys
//! and trailing commas — upstream D2RMM parses them with JSON5 for exactly that
//! reason. Writing is a different matter: output must be strict JSON (keys quoted,
//! no comments) because it goes back to the game, so parse and encode are
//! deliberately asymmetric.
//!
//! Output is minified and newline-terminated, matching `JSON.stringify` plus a
//! trailing newline upstream. Pretty-printing would be friendlier to read in a diff
//! and would also change every byte of every file a mod touches.

use std::path::Path;

use crate::error::{Error, Result};

/// Parses a game JSON file: JSON5 syntax, BOM tolerated.
pub fn parse(text: &str) -> serde_json::Result<serde_json::Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    json5::from_str(text).map_err(|e| serde_json::Error::io(std::io::Error::other(e.to_string())))
}

/// Serialises to the game's own style: compact JSON, one trailing newline.
pub fn encode(value: &serde_json::Value) -> Result<String> {
    let text = serde_json::to_string(value)
        .map_err(|e| Error::msg(format!("could not encode JSON: {e}")))?;
    Ok(text + "\n")
}

/// Reads and parses a file, naming it in any error.
pub fn read_file(path: &Path) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    parse(&text).map_err(|e| Error::msg(format!("{}: invalid JSON: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_comments_and_trailing_commas() {
        let value = parse("{\n  // a comment\n  a: 1,\n  b: [1, 2,],\n}").unwrap();
        assert_eq!(value, json!({"a": 1, "b": [1, 2]}));
    }

    #[test]
    fn accepts_bom() {
        assert_eq!(parse("\u{feff}{\"a\":1}").unwrap(), json!({"a": 1}));
    }

    #[test]
    fn encodes_compactly_with_newline() {
        let value = json!({"a": 1, "b": [1, 2]});
        assert_eq!(encode(&value).unwrap(), "{\"a\":1,\"b\":[1,2]}\n");
    }

    #[test]
    fn key_order_survives_a_round_trip() {
        let text = r#"{"zeta":1,"alpha":2,"mu":3}"#;
        let value = parse(text).unwrap();
        assert_eq!(encode(&value).unwrap(), format!("{text}\n"));
    }
}
