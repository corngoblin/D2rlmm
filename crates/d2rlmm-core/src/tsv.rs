//! D2R's `.txt` tables.
//!
//! Every game data table is tab-separated with a header row. On the way *in*, all
//! values are strings — `"1"`, not `1` — because that is what the file contains and
//! what existing mods compare against (`row.stackable === '1'`).
//!
//! On the way *out*, values are coerced to text rather than rejected. Real mods
//! assign numbers and booleans to cells all the time (`row.maxstack = config.size`,
//! `row.enabled = 1`), and upstream accepts that; refusing it here would break mods
//! that work under D2RMM. So the row type is [`serde_json::Value`] and [`Tsv::encode`]
//! stringifies the way JavaScript would.
//!
//! Two further details are load-bearing:
//!
//! * Blank lines are separators inside some tables and must not become empty rows.
//! * Files use CRLF. Reading with `remove_carriage_returns` strips the `\r` so the
//!   last column is usable; writing with `add_carriage_returns` puts them back. A
//!   mod that does one without the other produces a file D2R quietly mis-parses.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type TsvRow = IndexMap<String, Value>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Tsv {
    pub headers: Vec<String>,
    pub rows: Vec<TsvRow>,
}

impl Tsv {
    /// Parses a table. `remove_carriage_returns` strips `\r` from headers and values.
    pub fn parse(text: &str, remove_carriage_returns: bool) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = text.split('\n');

        let scrub = |s: &str| -> String {
            if remove_carriage_returns {
                s.trim_end_matches('\r').to_string()
            } else {
                s.to_string()
            }
        };

        let headers: Vec<String> = match lines.next() {
            Some(line) => line.split('\t').map(scrub).collect(),
            None => Vec::new(),
        };

        let mut rows = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let mut row = TsvRow::with_capacity(headers.len());
            for (index, value) in line.split('\t').enumerate() {
                // A row longer than the header is malformed; dropping the overflow
                // matches upstream and avoids inventing columns.
                let Some(header) = headers.get(index) else {
                    break;
                };
                row.insert(header.clone(), Value::String(scrub(value)));
            }
            rows.push(row);
        }

        Self { headers, rows }
    }

    /// Encodes the table. `add_carriage_returns` rewrites `\n` as `\r\n`, which is
    /// what the game's own files use.
    pub fn encode(&self, add_carriage_returns: bool) -> String {
        let mut out = String::new();
        out.push_str(&self.headers.join("\t"));
        out.push('\n');
        for row in &self.rows {
            let mut first = true;
            for header in &self.headers {
                if !first {
                    out.push('\t');
                }
                first = false;
                // A column the row does not mention is written as empty, never
                // skipped: a shifted column silently corrupts the whole table.
                if let Some(value) = row.get(header) {
                    out.push_str(&cell(value));
                }
            }
            out.push('\n');
        }
        if add_carriage_returns {
            out.replace('\n', "\r\n")
        } else {
            out
        }
    }
}

/// Renders a cell the way `String(value)` would, which is what upstream's
/// `headers.map(header => row[header] ?? '')` effectively does for the types mods
/// actually use.
fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                int.to_string()
            } else if let Some(uint) = number.as_u64() {
                uint.to_string()
            } else {
                let float = number.as_f64().unwrap_or(0.0);
                // `1.0` must not become "1.0": the game parses these as integers.
                if float.fract() == 0.0 && float.abs() < 1e15 {
                    format!("{}", float as i64)
                } else {
                    float.to_string()
                }
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_at<'a>(tsv: &'a Tsv, row: usize, header: &str) -> &'a str {
        tsv.rows[row][header].as_str().unwrap_or("")
    }

    #[test]
    fn parses_headers_and_rows() {
        let tsv = Tsv::parse("name\tmaxstack\r\nGold\t100\r\nKey\t12\r\n", true);
        assert_eq!(tsv.headers, ["name", "maxstack"]);
        assert_eq!(tsv.rows.len(), 2);
        assert_eq!(text_at(&tsv, 0, "name"), "Gold");
        // The point of remove_carriage_returns: the last column is clean.
        assert_eq!(text_at(&tsv, 1, "maxstack"), "12");
    }

    #[test]
    fn keeps_carriage_returns_when_asked() {
        // Not a bug: without `remove_carriage_returns` the final header keeps its
        // `\r` as well, which is exactly why mods are told to pass the option.
        let tsv = Tsv::parse("a\tb\r\n1\t2\r\n", false);
        assert_eq!(tsv.headers[1], "b\r");
        assert_eq!(text_at(&tsv, 0, "b\r"), "2\r");
    }

    #[test]
    fn skips_blank_separator_lines() {
        let tsv = Tsv::parse("a\tb\n1\t2\n\n3\t4\n", true);
        assert_eq!(tsv.rows.len(), 2);
    }

    #[test]
    fn round_trips_exactly() {
        let original = "name\tmaxstack\nGold\t100\nKey\t12\n";
        let tsv = Tsv::parse(original, true);
        assert_eq!(tsv.encode(false), original);
    }

    #[test]
    fn round_trips_with_crlf() {
        let original = "name\tmaxstack\r\nGold\t100\r\n";
        let tsv = Tsv::parse(original, true);
        assert_eq!(tsv.encode(true), original);
    }

    #[test]
    fn missing_cells_become_empty_not_absent() {
        let tsv = Tsv::parse("a\tb\tc\n1\t2\n", true);
        assert_eq!(tsv.encode(false), "a\tb\tc\n1\t2\t\n");
    }

    #[test]
    fn numbers_and_booleans_are_coerced_like_javascript() {
        // The shape SocketPunching and IncreaseStackSize actually produce.
        let mut tsv = Tsv::parse("description\tenabled\tnuminputs\n", true);
        let mut row = TsvRow::new();
        row.insert("description".into(), Value::from("Empty Sockets"));
        row.insert("enabled".into(), serde_json::json!(1));
        row.insert("numinputs".into(), serde_json::json!(2.0));
        tsv.rows.push(row);
        assert_eq!(
            tsv.encode(false),
            "description\tenabled\tnuminputs\nEmpty Sockets\t1\t2\n"
        );
    }

    #[test]
    fn columns_a_row_does_not_set_are_written_empty() {
        let mut tsv = Tsv::parse("a\tb\tc\n1\t2\t3\n", true);
        tsv.rows[0].shift_remove("b");
        assert_eq!(tsv.encode(false), "a\tb\tc\n1\t\t3\n");
    }
}
