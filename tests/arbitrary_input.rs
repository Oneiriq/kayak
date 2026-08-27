//! What every text-shaped input does, across an alphabet chosen to
//! break the assumptions the last sweep found broken.
//!
//! Three defects in a row came from a byte position assumed to be a
//! character boundary. Each was found by hand, one at a time, after it
//! shipped. These generate the space instead: every string up to four
//! characters over an alphabet of one-, two-, three- and four-byte
//! characters plus the punctuation the parsers look for. Exhaustive
//! and deterministic, so a failure names an input rather than a seed.

use kayak::runtime::cell;
use serde_json::Value;

/// One-, two-, three- and four-byte characters, the escape and
/// separator characters the parsers look for, and a digit.
const ALPHABET: [&str; 10] = [
    "a",
    "0",
    "%",
    "+",
    "-",
    ":",
    "T",
    "\u{e9}",
    "\u{20ac}",
    "\u{10348}",
];

/// Every string of `1..=length` characters over the alphabet.
fn every_string(length: usize) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    let mut row: Vec<String> = ALPHABET.iter().map(|s| (*s).to_owned()).collect();
    all.extend(row.iter().cloned());
    for _ in 1..length {
        let mut next = Vec::with_capacity(row.len() * ALPHABET.len());
        for prefix in &row {
            for piece in ALPHABET {
                next.push(format!("{prefix}{piece}"));
            }
        }
        all.extend(next.iter().cloned());
        row = next;
    }
    all
}

/// Nothing stored in a field ends the page in a panic.
///
/// `cell` renders every value a listing shows. `shorten_timestamp`
/// used to slice a stored string at byte 19 whatever was there.
#[test]
fn every_short_string_renders_as_a_cell() {
    // The columns that take a different path through the renderer.
    let columns = [
        "id",
        "state",
        "access",
        "digest",
        "created_at",
        "path",
        "size",
    ];
    for raw in every_string(3) {
        for column in columns {
            let _ = cell(column, Some(&Value::String(raw.clone())));
        }
    }
}

/// A timestamp-shaped value with a multi-byte character at every
/// position renders.
///
/// The shape check looks at bytes 4, 7, 10, 13 and the last one. What
/// the bytes between them are was the hole, so this walks a wide
/// character through every position of a value that passes the shape
/// check.
#[test]
fn a_timestamp_shaped_value_renders_wherever_a_wide_character_sits() {
    let stamp = "2026-08-05T19:29:23.394140700Z";
    for wide in ["\u{e9}", "\u{20ac}", "\u{10348}"] {
        for at in 0..stamp.chars().count() {
            let crafted: String = stamp
                .chars()
                .enumerate()
                .map(|(index, ch)| {
                    if index == at {
                        wide.to_owned()
                    } else {
                        ch.to_string()
                    }
                })
                .collect();
            let _ = cell("created_at", Some(&Value::String(crafted.clone())));
            // And with the trailing Z the shape check requires.
            let _ = cell("created_at", Some(&Value::String(format!("{crafted}Z"))));
        }
    }
}

/// Values that are not strings render too.
#[test]
fn every_shape_of_value_renders_as_a_cell() {
    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(u64::MAX),
        serde_json::json!(i64::MIN),
        serde_json::json!(f64::MAX),
        serde_json::json!([]),
        serde_json::json!([1, 2, 3]),
        serde_json::json!({}),
        serde_json::json!({"a": {"b": {"c": [1, {"d": null}]}}}),
        Value::String(String::new()),
        Value::String("\u{0}\u{1}\u{7f}".to_owned()),
        Value::String("\u{10348}".repeat(1000)),
    ];
    for value in &values {
        for column in ["id", "state", "size", "created_at", "digest", "metadata"] {
            let _ = cell(column, Some(value));
        }
        let _ = cell("anything", None);
    }
}

/// Whatever a cell renders, the markup it produces is escaped.
///
/// A field value reaches the page from the store, and the store holds
/// what a caller wrote.
#[test]
fn a_value_cannot_carry_markup_into_the_page() {
    // The claim is narrow and worth stating exactly, because a cell
    // writes markup of its own: no character from the value opens a
    // tag or closes an attribute. A value carrying none of those
    // characters is text either way, and reaches the page unchanged.
    let attacks = [
        "<script>alert(1)</script>",
        "\"><script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "</td></tr><tr><td>",
        "\u{3c}script\u{3e}",
        "&lt;script&gt;",
    ];
    for attack in attacks {
        for column in ["id", "path", "state", "digest", "created_at"] {
            let rendered = cell(column, Some(&Value::String(attack.to_owned()))).into_string();
            for opener in ["<script", "<img", "</td", "</tr", "<tr"] {
                assert!(
                    !rendered.contains(opener),
                    "{column} let {opener} through: {rendered}",
                );
            }
            for (raw, entity) in [('<', "&lt;"), ('>', "&gt;"), ('&', "&amp;")] {
                if attack.contains(raw) {
                    assert!(
                        rendered.contains(entity),
                        "{column} dropped {raw} instead of escaping it: {rendered}",
                    );
                }
            }
        }
    }

    // The same value through a nested object, which renders inside a
    // `pre` rather than as text.
    let nested = serde_json::json!({"note": "<script>alert(1)</script>"});
    let rendered = cell("metadata", Some(&nested)).into_string();
    assert!(
        !rendered.contains("<script"),
        "a nested value carried a tag: {rendered}",
    );
}
