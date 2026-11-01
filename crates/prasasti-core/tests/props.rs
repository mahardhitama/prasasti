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

/// Positions run past the end of the text, and `a..b` may be reversed.
fn range() -> impl Strategy<Value = Range> {
    (0usize..50, 0usize..50).prop_map(|(a, b)| a..b)
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
        t in text(),
        sel in range(),
        marker in proptest::sample::select(vec!["**", "*", "~~", "_"]),
    ) {
        in_bounds(&t, &toggle_emphasis(&t, sel, marker));
    }

    #[test]
    fn insert_link_in_bounds(t in text(), sel in range(), url in ".{0,20}") {
        in_bounds(&t, &insert_link(&t, sel, &url));
    }

    #[test]
    fn indent_in_bounds(t in text(), sel in range(), outdent in any::<bool>()) {
        if let Some(e) = list::indent(&t, sel, outdent) {
            in_bounds(&t, &e);
        }
    }

    #[test]
    fn count_words_never_panics(t in text()) {
        count_words(&t);
    }
}
