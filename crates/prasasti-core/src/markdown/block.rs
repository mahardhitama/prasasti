//! Block state and per-paragraph styling. The editor feeds one paragraph at a
//! time and gets back spans to apply in order.

use super::inline::{ascii_space, code_spans, inline_markup, is_thematic_break, InlineKind};
use crate::Range;

const BACKTICK: u16 = b'`' as u16;
const TILDE: u16 = b'~' as u16;
const SPACE: u16 = b' ' as u16;
const HASH: u16 = b'#' as u16;
const QUOTE: u16 = b'>' as u16;
const DASH: u16 = b'-' as u16;
const PLUS: u16 = b'+' as u16;
const STAR: u16 = b'*' as u16;
const DOT: u16 = b'.' as u16;
const PAREN: u16 = b')' as u16;
/// Inline styling is skipped for text with none of these.
const INLINE_TRIGGERS: [u16; 5] = [BACKTICK, STAR, b'_' as u16, b'[' as u16, TILDE];

/// What a paragraph hands to the next one.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum BlockState {
    #[default]
    Prose,
    /// Inside a fenced code block opened by `len` copies of `marker`.
    InsideFence { marker: u16, len: usize },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StyleKind {
    Heading(u8),
    Bold,
    Italic,
    Strike,
    Code,
    Fence,
    Quote,
    Link,
    Marker,
    HiddenMarker,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StyleSpan {
    pub range: Range,
    pub kind: StyleKind,
}

/// Length of the run of `c` at the start of `t`.
fn run(t: &[u16], c: u16) -> usize {
    t.iter().take_while(|&&u| u == c).count()
}

/// `^ {0,3}(?:(`{3,})[^`]*|(~{3,}).*)$`: the fence character and its length.
/// A backtick fence's info string may not hold a backtick: "```code```" is
/// inline code.
pub fn opening_fence(text: &[u16]) -> Option<(u16, usize)> {
    let t = &text[run(text, SPACE).min(3)..];
    let marker = *t.first().filter(|&&u| u == BACKTICK || u == TILDE)?;
    let len = run(t, marker);
    (len >= 3 && (marker == TILDE || !t[len..].contains(&BACKTICK))).then_some((marker, len))
}

/// ``^ {0,3}(`{3,}|~{3,})\s*$``, with at least `len` of the same character.
pub fn closes_fence(text: &[u16], marker: u16, len: usize) -> bool {
    let t = &text[run(text, SPACE).min(3)..];
    let n = run(t, marker);
    n >= len.max(3) && (marker == BACKTICK || marker == TILDE) && t[n..].iter().all(|&u| ascii_space(u))
}

fn is_space(u: u16) -> bool {
    char::from_u32(u as u32).is_some_and(char::is_whitespace)
}

fn span(range: Range, kind: StyleKind) -> Option<StyleSpan> {
    (!range.is_empty()).then_some(StyleSpan { range, kind })
}

/// Number of leading ASCII whitespace units from `from`.
fn spaces(t: &[u16], from: usize) -> usize {
    t[from..].iter().take_while(|&&u| ascii_space(u)).count()
}

/// Heading, quote, list and rule markers.
fn block_spans(t: &[u16], out: &mut Vec<StyleSpan>) {
    let Some(first) = t.iter().position(|&u| !is_space(u)) else { return };
    let len = t.len();

    if first == 0 && t[0] == HASH {
        let hashes = run(t, HASH);
        let ws = if hashes <= 6 { spaces(t, hashes) } else { 0 };
        if (1..=6).contains(&hashes) && ws > 0 {
            let start = hashes + ws;
            out.extend(span(0..start, StyleKind::Marker));
            out.extend(span(start..len, StyleKind::Heading(hashes as u8)));
            return;
        }
    }

    if t[first] == QUOTE {
        // `^(\s*>+\s?)(.*)$`
        let lead = spaces(t, 0);
        if t.get(lead) == Some(&QUOTE) {
            let mut end = lead + run(&t[lead..], QUOTE);
            end += t.get(end).is_some_and(|&u| ascii_space(u)) as usize;
            out.extend(span(0..end, StyleKind::Marker));
            out.extend(span(end..len, StyleKind::Quote));
        }
    }

    // `^(\s*(?:[-+*]|\d+[.)])\s+)(.*)$`
    let lead = spaces(t, 0);
    let digits = t[lead..].iter().take_while(|&&u| (b'0' as u16..=b'9' as u16).contains(&u)).count();
    let after = match t.get(lead) {
        Some(&u) if [DASH, PLUS, STAR].contains(&u) => Some(lead + 1),
        Some(_) if digits > 0 && t.get(lead + digits).is_some_and(|&u| u == DOT || u == PAREN) => {
            Some(lead + digits + 1)
        }
        _ => None,
    };
    if let Some(after) = after {
        let ws = spaces(t, after);
        if ws > 0 {
            out.extend(span(0..after + ws, StyleKind::Marker));
        }
    }

    if is_thematic_break(t) {
        out.extend(span(0..len, StyleKind::Marker));
    }
}

/// Inline markup spans. Spans may overlap; a later span adds to or
/// overrides an earlier one, so enclosing markup comes before what it holds.
fn inline_spans(t: &[u16], caret: Option<usize>, out: &mut Vec<StyleSpan>) {
    // Code keeps its own styling even when emphasis or a link encloses it.
    let mut in_code = vec![false; t.len()];
    for code in code_spans(t) {
        for i in code.clone() {
            in_code[i] = true;
        }
        out.extend(span(code, StyleKind::Code));
    }

    // Outer markup first, so what it encloses has the last word.
    let mut markup = inline_markup(t);
    markup.sort_by(|a, b| b.content.len().cmp(&a.content.len()));
    for item in &markup {
        let kind = match item.kind {
            InlineKind::Bold => StyleKind::Bold,
            InlineKind::Italic => StyleKind::Italic,
            InlineKind::Strike => StyleKind::Strike,
            InlineKind::Link => StyleKind::Link,
        };
        if item.kind == InlineKind::Strike {
            out.extend(span(item.content.clone(), kind));
            continue;
        }
        // Everything but strikethrough skips code: emit the runs between.
        let mut start = item.content.start;
        for i in item.content.clone().chain(std::iter::once(item.content.end)) {
            if i == item.content.end || in_code.get(i).copied().unwrap_or(false) {
                out.extend(span(start..i, kind));
                start = i + 1;
            }
        }
    }

    // Last, so no enclosing style shows a marker that should be hidden.
    for item in &markup {
        let at_caret = caret.is_some_and(|c| c >= item.markers[0].start && c <= item.markers[1].end);
        let kind = if at_caret { StyleKind::Marker } else { StyleKind::HiddenMarker };
        for marker in &item.markers {
            out.extend(span(marker.clone(), kind));
        }
    }
}

/// Styles one paragraph (no trailing newline). `caret` is the caret's offset
/// in this paragraph, or `None` when it is elsewhere. Returns spans in
/// application order, block spans first, and the state for the next paragraph.
pub fn style_paragraph(text: &[u16], prev: BlockState, caret: Option<usize>) -> (Vec<StyleSpan>, BlockState) {
    let whole = |kind| span(0..text.len(), kind).into_iter().collect::<Vec<_>>();

    // A fenced run of code is literal from its opening fence through its
    // closing one, so no markup runs inside it.
    if let BlockState::InsideFence { marker, len } = prev {
        return if closes_fence(text, marker, len) {
            (whole(StyleKind::Fence), BlockState::Prose)
        } else {
            (whole(StyleKind::Code), prev)
        };
    }
    if let Some((marker, len)) = opening_fence(text) {
        return (whole(StyleKind::Fence), BlockState::InsideFence { marker, len });
    }

    let mut spans = Vec::new();
    block_spans(text, &mut spans);
    if text.iter().any(|u| INLINE_TRIGGERS.contains(u)) {
        inline_spans(text, caret, &mut spans);
    }
    (spans, BlockState::Prose)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn styled(text: &str, caret: Option<usize>) -> Vec<StyleSpan> {
        style_paragraph(&u(text), BlockState::Prose, caret).0
    }

    fn covers(spans: &[StyleSpan], kind: StyleKind, at: usize) -> bool {
        spans.iter().any(|s| s.kind == kind && s.range.contains(&at))
    }

    fn hidden_count(spans: &[StyleSpan]) -> usize {
        (0..200).filter(|&i| covers(spans, StyleKind::HiddenMarker, i)).count()
    }

    /// Feeds lines in order, threading the state; returns each line's state and spans.
    fn run_lines(text: &str) -> Vec<(BlockState, Vec<StyleSpan>)> {
        let mut state = BlockState::Prose;
        text.split('\n')
            .map(|line| {
                let (spans, next) = style_paragraph(&u(line), state, None);
                state = next;
                (next, spans)
            })
            .collect()
    }

    fn fence(marker: char, len: usize) -> BlockState {
        BlockState::InsideFence { marker: marker as u16, len }
    }

    #[test]
    fn leaves_fenced_code_literal() {
        let lines = run_lines("prose _italic_\n```ruby\nsnake_case_name = *value*\n```\nafter _italic_");
        let states: Vec<_> = lines.iter().map(|l| l.0).collect();
        assert_eq!(
            states,
            [BlockState::Prose, fence('`', 3), fence('`', 3), BlockState::Prose, BlockState::Prose]
        );
        // Fenced line: one Code span over the whole line, nothing else.
        assert_eq!(lines[2].1, [StyleSpan { range: 0..25, kind: StyleKind::Code }]);
        assert_eq!(lines[1].1, [StyleSpan { range: 0..7, kind: StyleKind::Fence }]);
        assert_eq!(lines[3].1, [StyleSpan { range: 0..3, kind: StyleKind::Fence }]);
        assert!(covers(&lines[4].1, StyleKind::Italic, 7));
    }

    #[test]
    fn opens_fences_only_where_markdown_does() {
        let lines = run_lines("```inline``` and _italic_\n````md\n```\nstill code\n````\nafter _italic_");
        assert_eq!(lines[0].0, BlockState::Prose);
        assert_eq!(lines[1].0, fence('`', 4));
        assert_eq!(lines[2].0, fence('`', 4));
        assert_eq!(lines[3].0, fence('`', 4));
        assert_eq!(lines[4].0, BlockState::Prose);
        assert_eq!(lines[5].0, BlockState::Prose);

        assert_eq!(opening_fence(&u("```ruby")), Some((b'`' as u16, 3)));
        assert_eq!(opening_fence(&u("    ```")), None);
        assert_eq!(opening_fence(&u("   ```")), Some((b'`' as u16, 3)));
        assert_eq!(opening_fence(&u("``")), None);
        assert!(!closes_fence(&u("``` text"), b'`' as u16, 3));
        assert!(closes_fence(&u("```  "), b'`' as u16, 3));
        assert!(closes_fence(&u("`````"), b'`' as u16, 4));
        assert!(!closes_fence(&u("~~~"), b'`' as u16, 3));
    }

    #[test]
    fn recognises_tilde_fences() {
        let lines = run_lines("~~~js\n*code*\n```\n~~~\nafter *it*");
        let states: Vec<_> = lines.iter().map(|l| l.0).collect();
        assert_eq!(
            states,
            [fence('~', 3), fence('~', 3), fence('~', 3), BlockState::Prose, BlockState::Prose]
        );
        assert!(covers(&lines[4].1, StyleKind::Italic, 7));
    }

    #[test]
    fn hides_no_markers_inside_code() {
        let lines = run_lines(
            "``` **opening**\n# _literal_ [link](url)\n```  \n*prose*\n`*literal*`\n```\n_unterminated_",
        );
        for n in [0, 1, 2, 4, 5, 6] {
            assert_eq!(hidden_count(&lines[n].1), 0, "line {n}");
        }
        assert_eq!(hidden_count(&lines[3].1), 2);

        // Without the unmatched fence the last line is prose again.
        let lines = run_lines("``` **opening**\n# _literal_ [link](url)\n```  \n*prose*\n`*literal*`\n\n_unterminated_");
        assert_eq!(hidden_count(&lines[6].1), 2);
    }

    #[test]
    fn shows_markers_around_the_caret() {
        let text = "a **b** c";
        for caret in [0, 1, 8, 9] {
            let spans = styled(text, Some(caret));
            for i in [2, 3, 5, 6] {
                assert!(covers(&spans, StyleKind::HiddenMarker, i), "caret {caret} at {i}");
                assert!(!covers(&spans, StyleKind::Marker, i));
            }
        }
        for caret in [2, 3, 4, 5, 7] {
            let spans = styled(text, Some(caret));
            for i in [2, 3, 5, 6] {
                assert!(covers(&spans, StyleKind::Marker, i), "caret {caret} at {i}");
                assert!(!covers(&spans, StyleKind::HiddenMarker, i));
            }
            assert!(covers(&spans, StyleKind::Bold, 4));
        }

        // Everything the caret sits in shows, and nothing beside it.
        let spans = styled("*a __b__ c* _d_", Some(5));
        for i in [0, 3, 4, 6, 7, 10] {
            assert!(covers(&spans, StyleKind::Marker, i) && !covers(&spans, StyleKind::HiddenMarker, i), "{i}");
        }
        for i in [12, 14] {
            assert!(covers(&spans, StyleKind::HiddenMarker, i), "{i}");
        }

        // Another paragraph's caret hides them; this one's shows them.
        assert!(covers(&styled("**a**", Some(1)), StyleKind::Marker, 0));
        assert!(covers(&styled("**a**", None), StyleKind::HiddenMarker, 0));
    }

    #[test]
    fn hides_markers_inside_nested_markup() {
        let linked = styled("[**a**](u)", None);
        for i in [0, 1, 2, 4, 5, 6] {
            assert!(covers(&linked, StyleKind::HiddenMarker, i), "{i}");
        }
        assert!(covers(&linked, StyleKind::Bold, 3));
        assert!(covers(&linked, StyleKind::Link, 3));

        let nested = styled("*a __b__ c*", None);
        for i in [0, 3, 4, 6, 7, 10] {
            assert!(covers(&nested, StyleKind::HiddenMarker, i), "{i}");
        }
        assert!(covers(&nested, StyleKind::Italic, 5));
        assert!(covers(&nested, StyleKind::Bold, 5));
    }

    #[test]
    fn renders_bold_italic_runs() {
        let spans = styled("a ***word*** b", None);
        assert!(covers(&spans, StyleKind::Italic, 5));
        assert!(covers(&spans, StyleKind::Bold, 5));
        for i in [2, 3, 4, 9, 10, 11] {
            assert!(covers(&spans, StyleKind::HiddenMarker, i), "{i}");
        }
    }

    #[test]
    fn leaves_thematic_breaks_alone() {
        for rule in ["*****", "___", "* * *"] {
            let spans = styled(rule, None);
            for i in 0..rule.len() {
                assert!(!covers(&spans, StyleKind::Bold, i), "{rule} {i}");
                assert!(!covers(&spans, StyleKind::HiddenMarker, i), "{rule} {i}");
            }
            assert!(covers(&spans, StyleKind::Marker, 0), "{rule}");
        }
        assert!(is_thematic_break(&u("---")));
        assert!(!is_thematic_break(&u("--")));
    }

    #[test]
    fn merges_inline_styles_with_the_block() {
        let heading = styled("# head *it* [l](u) `c`", None);
        assert!(covers(&heading, StyleKind::Italic, 8));
        for i in [8, 13, 20] {
            assert!(covers(&heading, StyleKind::Heading(1), i), "{i}");
        }
        assert!(covers(&heading, StyleKind::Link, 13));
        assert!(covers(&heading, StyleKind::Code, 20));
        assert!(covers(&heading, StyleKind::Marker, 0) && covers(&heading, StyleKind::Marker, 1));
        assert!(!covers(&heading, StyleKind::Heading(1), 1));

        let quote = styled("> quote **b**", None);
        assert!(covers(&quote, StyleKind::Bold, 10));
        assert!(covers(&quote, StyleKind::Quote, 10) && covers(&quote, StyleKind::Quote, 3));
        assert!(covers(&quote, StyleKind::Marker, 0) && covers(&quote, StyleKind::Marker, 1));
        // Block spans come first.
        let first_inline = quote.iter().position(|s| s.kind == StyleKind::Bold).unwrap();
        assert!(quote[..first_inline].iter().all(|s| matches!(s.kind, StyleKind::Marker | StyleKind::Quote)));
    }

    #[test]
    fn keeps_enclosed_code_styling() {
        for (text, kind) in [
            ("*a `code` b*", StyleKind::Italic),
            ("**a `code` b**", StyleKind::Bold),
            ("[a `code` b](https://example.com)", StyleKind::Link),
        ] {
            let spans = styled(text, None);
            let a = text.find('a').unwrap();
            for other in [StyleKind::Italic, StyleKind::Bold, StyleKind::Link] {
                assert_eq!(covers(&spans, other, a), other == kind, "{text}");
            }
            assert!(!covers(&spans, StyleKind::Code, a));
            let code = text.find('`').unwrap();
            for i in code..code + 6 {
                assert!(covers(&spans, StyleKind::Code, i), "{text} {i}");
                for other in [StyleKind::Italic, StyleKind::Bold, StyleKind::Link] {
                    assert!(!covers(&spans, other, i), "{text} {i}");
                }
            }
            let markers: Vec<_> = spans.iter().filter(|s| s.kind == StyleKind::HiddenMarker).collect();
            assert_eq!(markers.len(), 2, "{text}");
        }
    }

    #[test]
    fn styles_an_enclosed_code_span_as_code() {
        let spans = styled("_a `b` c_", None);
        assert!(covers(&spans, StyleKind::Italic, 1));
        assert!(!covers(&spans, StyleKind::Code, 1));
        assert!(covers(&spans, StyleKind::Code, 4));
        assert!(!covers(&spans, StyleKind::Italic, 4));
        assert!(covers(&spans, StyleKind::Italic, 7));
    }

    #[test]
    fn strike_draws_over_code() {
        let spans = styled("~~a `b` c~~", None);
        assert!(covers(&spans, StyleKind::Strike, 4) && covers(&spans, StyleKind::Code, 4));
    }

    #[test]
    fn lists_and_rules_get_markers() {
        for (text, end) in [("- item", 2), ("  12. item", 6), ("+ x", 2), ("1) x", 3)] {
            let spans = styled(text, None);
            assert_eq!(spans, [StyleSpan { range: 0..end, kind: StyleKind::Marker }], "{text}");
        }
        assert!(styled("-item", None).is_empty());
        assert!(styled("####### x", None).is_empty());
        assert!(styled("#x", None).is_empty());
        assert!(styled("", Some(0)).is_empty());
    }

    #[test]
    fn never_panics_on_odd_input() {
        let junk = [
            "", "#", "# ", ">", ">>", "> ", "`", "```", "~~~", "*", "**", "\u{FFFC}", "[](", "- ", "1.", "\u{1F600}**a**",
        ];
        for text in junk {
            let t = u(text);
            for prev in [BlockState::Prose, fence('`', 3), fence('~', 0)] {
                for caret in [None, Some(0), Some(1), Some(1000)] {
                    let (spans, _) = style_paragraph(&t, prev, caret);
                    assert!(spans.iter().all(|s| s.range.start < s.range.end && s.range.end <= t.len()), "{text}");
                }
            }
        }
    }
}
