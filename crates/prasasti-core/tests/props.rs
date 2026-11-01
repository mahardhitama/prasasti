//! No-panic and bounds properties over arbitrary text, including emoji, lone
//! `\r`, lone surrogates and out-of-range positions.

use prasasti_core::edit::{insert_link, toggle_emphasis, Edit};
use prasasti_core::markdown::{style_paragraph, BlockState};
use prasasti_core::search::find_matches;
use prasasti_core::words::count_words;
use prasasti_core::{list, Range};
use proptest::prelude::*;

const MARKERS: &str = "*_`~[]()#>-0123456789 \t\n\r";

/// Marker-heavy strings with emoji and combining text, or raw UTF-16 units
/// that may hold lone surrogates.
fn text() -> impl Strategy<Value = Vec<u16>> {
    let piece = prop_oneof![
        4 => proptest::sample::select(MARKERS.chars().collect::<Vec<_>>()).prop_map(String::from),
        1 => proptest::sample::select(vec!["😀", "é", "e\u{301}", "a", "word", "https://x.io"])
            .prop_map(String::from),
    ];
    let string = prop::collection::vec(piece, 0..40)
        .prop_map(|v| v.concat().encode_utf16().collect::<Vec<u16>>());
    let unit = prop_oneof![
        3 => proptest::sample::select(MARKERS.encode_utf16().collect::<Vec<_>>()),
        1 => 0xD800u16..=0xDFFF,
        1 => any::<u16>(),
    ];
    prop_oneof![3 => string, 1 => prop::collection::vec(unit, 0..40)]
}

/// Positions run 0..=len+2, so past the end, and `a..b` may be reversed.
fn with_range(t: impl Strategy<Value = Vec<u16>>) -> impl Strategy<Value = (Vec<u16>, Range)> {
    t.prop_flat_map(|t| {
        let max = t.len() + 2;
        (Just(t), 0..=max, 0..=max).prop_map(|(t, a, b)| (t, a..b))
    })
}

/// Mostly list lines (indent of spaces/tabs, marker, space, text), with the
/// odd blank, prose, rule or fence line, so `indent` often takes the edit.
fn listy() -> impl Strategy<Value = Vec<u16>> {
    let indent = proptest::sample::select(vec!["", "", " ", "  ", "    ", "\t", " \t", "        "]);
    let marker = proptest::sample::select(vec!["-", "+", "*", "1.", "10)", "99."]);
    let item = (indent, marker, proptest::sample::select(vec!["a", "😀 b", "", "[l](u)"]))
        .prop_map(|(i, m, t)| format!("{i}{m} {t}"));
    let other = proptest::sample::select(vec!["", " ", "  more", "Para", "```", "~~~", "* * *", "- - -"])
        .prop_map(String::from);
    let lines = prop::collection::vec(prop_oneof![8 => item, 2 => other], 1..8);
    lines.prop_map(|v| v.join("\n").encode_utf16().collect())
}

fn in_bounds(text: &[u16], e: &Edit) {
    assert!(e.range.start <= e.range.end && e.range.end <= text.len(), "{:?}", e.range);
    let len = text.len() - e.range.len() + e.replacement.encode_utf16().count();
    assert!(e.selection.start <= e.selection.end && e.selection.end <= len, "{:?}", e.selection);
}

proptest! {
    #[test]
    fn style_paragraph_spans_in_text(
        t in text(),
        caret in proptest::option::of(0usize..50),
        fence in proptest::option::of((proptest::sample::select(vec![96u16, 126, 65]), 0usize..6)),
    ) {
        let state = fence.map_or(BlockState::Prose, |(marker, len)| BlockState::InsideFence { marker, len });
        let (spans, _) = style_paragraph(&t, state, caret);
        for s in spans {
            prop_assert!(s.range.start <= s.range.end && s.range.end <= t.len(), "{:?}", s);
        }
    }

    #[test]
    fn find_matches_in_text(t in text(), q in text()) {
        for m in find_matches(&t, &q) {
            prop_assert!(m + q.len() <= t.len());
        }
    }

    #[test]
    fn toggle_emphasis_in_bounds(
        (t, sel) in with_range(text()),
        marker in proptest::sample::select(vec!["**", "*", "~~", "_"]),
    ) {
        in_bounds(&t, &toggle_emphasis(&t, sel, marker));
    }

    #[test]
    fn insert_link_in_bounds((t, sel) in with_range(text()), url in ".{0,20}") {
        in_bounds(&t, &insert_link(&t, sel, &url));
    }

    #[test]
    fn indent_in_bounds((t, sel) in with_range(text()), outdent in any::<bool>()) {
        if let Some(e) = list::indent(&t, sel, outdent) {
            in_bounds(&t, &e);
        }
    }

    #[test]
    fn indent_in_bounds_on_lists((t, sel) in with_range(listy()), outdent in any::<bool>()) {
        if let Some(e) = list::indent(&t, sel, outdent) {
            in_bounds(&t, &e);
        }
    }

    #[test]
    fn count_words_never_panics(t in text()) {
        count_words(&t);
    }
}

/// The list property is only worth something if `indent` takes the edit, so
/// sweep every selection over a list text and require that it does.
#[test]
fn indent_takes_edits_on_lists() {
    let t: Vec<u16> = "- a\n    - b\n  more\n\n\t1. c\n```\n- x\n```\n* * *\n10) d"
        .encode_utf16()
        .collect();
    let mut some = 0;
    for a in 0..=t.len() + 2 {
        for b in 0..=t.len() + 2 {
            for outdent in [false, true] {
                if let Some(e) = list::indent(&t, a..b, outdent) {
                    some += 1;
                    in_bounds(&t, &e);
                }
            }
        }
    }
    assert!(some > 100, "only {some} edits");
}
