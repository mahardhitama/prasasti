//! Emphasis toggles and link insertion. Each returns an `Edit` for the app to
//! apply.

use crate::markdown::{inline_markup, style_paragraph, BlockState, InlineKind};
use crate::Range;

/// Replace `range` (pre-edit text) with `replacement`, then select `selection`
/// (post-edit text). A zero-length selection is a caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range,
    pub replacement: String,
    pub selection: Range,
}

/// The URL a clipboard holds, if it is one worth linking: the first line,
/// `www.` taken as https, and only http(s), ftp and mailto allowed.
pub fn normalized_link_url(s: &str) -> Option<String> {
    let line = s.trim().split(['\r', '\n']).next().unwrap_or("").trim();
    let url =
        if line.len() >= 4 && line.is_char_boundary(4) && line[..4].eq_ignore_ascii_case("www.") {
            format!("https://{line}")
        } else {
            line.to_string()
        };
    // `^[A-Za-z][A-Za-z0-9+.-]*:`
    let colon = url.find(':')?;
    let scheme = &url[..colon];
    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "mailto" {
        // shortcut: no further QUrl validity checks, add them if bad mailto links show up.
        return Some(format!("{scheme}{}", &url[colon..]));
    }
    if !["http", "https", "ftp"].contains(&scheme.as_str()) {
        return None;
    }
    let rest = url[colon + 1..].strip_prefix("//").unwrap_or("");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = if host.starts_with('[') {
        host
    } else {
        host.split(':').next().unwrap_or("")
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (userinfo, hostport) = authority.split_at(authority.rfind('@').map_or(0, |i| i + 1));
    Some(format!(
        "{scheme}://{userinfo}{}{}",
        hostport.to_ascii_lowercase(),
        &rest[authority.len()..]
    ))
}

fn clamp_selection(text: &[u16], selection: Range) -> (usize, usize) {
    let start = selection.start.min(text.len());
    (start, selection.end.clamp(start, text.len()))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Replaces `start..end` with `replacement` and selects `from..to` within it.
fn replace(start: usize, end: usize, replacement: String, from: usize, to: usize) -> Edit {
    let len = utf16_len(&replacement);
    Edit {
        range: start..end,
        replacement,
        selection: start + from.min(len)..start + to.min(len),
    }
}

/// A caret move with no text change.
fn caret_to(pos: usize) -> Edit {
    Edit {
        range: pos..pos,
        replacement: String::new(),
        selection: pos..pos,
    }
}

fn lossy(text: &[u16]) -> String {
    String::from_utf16_lossy(text)
}

/// Where the caret's emphasis span of one kind sits, in document offsets.
struct Span {
    start: usize,
    end: usize,
    content_start: usize,
    content_end: usize,
    /// The visible text ends before the chain of nested emphasis closing with
    /// it, as the italic "*" inside "***word***".
    text_end: usize,
}

/// The emphasis span of the marker's kind that `position` sits in or against.
fn emphasis_span_at(text: &[u16], position: usize, marker: &str) -> Option<Span> {
    let position = position.min(text.len());
    let nl = u16::from(b'\n');
    // Fenced code is literal: nothing in it, or on its fences, is emphasis.
    let mut state = BlockState::Prose;
    let mut line_start = 0;
    let line = loop {
        let line_end = text[line_start..]
            .iter()
            .position(|&c| c == nl)
            .map_or(text.len(), |i| line_start + i);
        let (_, next) = style_paragraph(&text[line_start..line_end], state, None);
        if position <= line_end || line_end == text.len() {
            if state != BlockState::Prose || next != BlockState::Prose {
                return None;
            }
            break line_start..line_end;
        }
        state = next;
        line_start = line_end + 1;
    };
    let kind = match marker {
        "**" => InlineKind::Bold,
        "~~" => InlineKind::Strike,
        _ => InlineKind::Italic,
    };
    let offset = position - line.start;
    let markup = inline_markup(&text[line.clone()]);
    let item = markup
        .iter()
        .find(|m| m.kind == kind && offset >= m.markers[0].start && offset <= m.markers[1].end)?;
    let content_end = item.content.end;
    let mut text_end = content_end;
    while let Some(nested) = markup.iter().find(|n| {
        let c = &n.markers[1];
        c.start >= item.content.start && !c.is_empty() && c.end == text_end
    }) {
        text_end = nested.markers[1].start;
    }
    let base = line.start;
    Some(Span {
        start: base + item.markers[0].start,
        end: base + item.markers[1].end,
        content_start: base + item.content.start,
        content_end: base + content_end,
        text_end: base + text_end,
    })
}

/// Unwraps when the marker is already there, selected or just outside the
/// selection, and wraps otherwise. A marker only counts as a whole run, so the
/// "*" of "**bold**" is not taken for italic; "***" counts as both.
///
/// With nothing selected it acts on the span around the caret: at the end of
/// its text the caret steps past the closing marker, so typing carries on
/// plain (and steps back in on a second press); anywhere else in it the span
/// comes off.
pub fn toggle_emphasis(text: &[u16], selection: Range, marker: &str) -> Edit {
    let (start, end) = clamp_selection(text, selection);
    if start == end {
        if let Some(span) = emphasis_span_at(text, start, marker) {
            if start == span.end {
                return caret_to(span.text_end);
            }
            if start >= span.text_end {
                return caret_to(span.end);
            }
            let caret = start.saturating_sub(span.content_start);
            return replace(
                span.start,
                span.end,
                lossy(&text[span.content_start..span.content_end]),
                caret,
                caret,
            );
        }
    }

    let marker16: Vec<u16> = marker.encode_utf16().collect();
    let size = marker16.len();
    let mark = marker16.first().copied().unwrap_or(0);
    let at = |i: isize| usize::try_from(i).ok().and_then(|i| text.get(i).copied());
    // Length of the run of `mark` from `position` stepping by `step`, stopping at `limit`.
    let run_from = |position: isize, step: isize, limit: isize| {
        let mut run = 0;
        while position + run * step != limit && at(position + run * step) == Some(mark) {
            run += 1;
        }
        run as usize
    };
    let holds = |run: usize| run == size || (mark == u16::from(b'*') && run == 3);
    let has_marker = |part: &[u16]| {
        part.split(|&c| c != mark)
            .any(|run| !run.is_empty() && holds(run.len()))
    };
    let (s, e, len) = (start as isize, end as isize, text.len() as isize);
    let selected = &text[start..end];

    // A marker in the middle means separate spans, as in "**a** and **b**":
    // unwrapping would break them, so wrap instead.
    if holds(run_from(s - 1, -1, -1)) && holds(run_from(e, 1, len)) && !has_marker(selected) {
        return replace(start - size, end + size, lossy(selected), 0, selected.len());
    }
    if selected.len() >= 2 * size
        && holds(run_from(s, 1, e))
        && holds(run_from(e - 1, -1, s - 1))
        && !has_marker(&selected[size..selected.len() - size])
        && at(s - 1) != Some(mark)
        && at(e) != Some(mark)
    {
        let inner = &selected[size..selected.len() - size];
        return replace(start, end, lossy(inner), 0, inner.len());
    }
    replace(
        start,
        end,
        format!("{marker}{}{marker}", lossy(selected)),
        size,
        size + selected.len(),
    )
}

fn escape(s: &str, specials: &[char]) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' || specials.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `[sel](url)` with the label selected, or `[sel]()` with the caret in the
/// parentheses when the clipboard holds no URL. With nothing selected the
/// label is empty and the caret waits in it.
pub fn insert_link(text: &[u16], selection: Range, clipboard: &str) -> Edit {
    let (start, end) = clamp_selection(text, selection);
    let label = escape(&lossy(&text[start..end]), &['[', ']']);
    let label_len = utf16_len(&label);
    let url = normalized_link_url(clipboard);
    let markdown = format!(
        "[{label}]({})",
        url.as_deref()
            .map_or(String::new(), |u| escape(u, &['(', ')']))
    );
    match (label_len, url) {
        (0, _) => replace(start, end, markdown, 1, 1),
        (_, Some(_)) => replace(start, end, markdown, 1, 1 + label_len),
        (_, None) => replace(start, end, markdown, label_len + 3, label_len + 3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn apply(text: &str, e: &Edit) -> String {
        let mut t = u(text);
        t.splice(e.range.clone(), e.replacement.encode_utf16());
        String::from_utf16(&t).unwrap()
    }

    fn toggle(text: &str, from: usize, to: usize, marker: &str) -> String {
        apply(text, &toggle_emphasis(&u(text), from..to, marker))
    }

    /// A caret-only editor: Ctrl+B / Ctrl+I and typed characters.
    struct Typist {
        text: String,
        caret: usize,
    }

    impl Typist {
        fn new(text: &str, caret: usize) -> Self {
            Typist {
                text: text.into(),
                caret,
            }
        }
        fn key(&mut self, marker: &str) {
            let e = toggle_emphasis(&u(&self.text), self.caret..self.caret, marker);
            self.text = apply(&self.text, &e);
            assert_eq!(
                e.selection.start, e.selection.end,
                "a caret toggle leaves a caret"
            );
            self.caret = e.selection.start;
        }
        fn bold(&mut self) {
            self.key("**");
        }
        fn type_(&mut self, s: &str) {
            let mut t = u(&self.text);
            t.splice(self.caret..self.caret, s.encode_utf16());
            self.text = String::from_utf16(&t).unwrap();
            self.caret += s.encode_utf16().count();
        }
    }

    #[test]
    fn normalizes_links() {
        let n = normalized_link_url;
        assert_eq!(
            n("www.example.com/path").as_deref(),
            Some("https://www.example.com/path")
        );
        assert_eq!(
            n("mailto:writer@example.com").as_deref(),
            Some("mailto:writer@example.com")
        );
        assert_eq!(n("example.com"), None);
        assert_eq!(n("file:///tmp/private"), None);
        assert_eq!(
            n("  https://example.com/a \nsecond line").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(n("https://"), None);
        assert_eq!(n(""), None);
        assert_eq!(n("javascript:alert(1)"), None);
    }

    #[test]
    fn toggles_emphasis_markers() {
        let (b, i) = ("**", "*");
        assert_eq!(toggle("a word b", 2, 6, b), "a **word** b");
        // Off again, whether the markers sit outside the selection or in it.
        assert_eq!(toggle("a **word** b", 4, 8, b), "a word b");
        assert_eq!(toggle("a **word** b", 2, 10, b), "a word b");
        assert_eq!(toggle("a ~~word~~ b", 4, 8, "~~"), "a word b");
        // An empty pair goes away too.
        assert_eq!(toggle("Hello****.", 7, 7, b), "Hello.");

        // The "*" of bold is not italic, and "***" holds both.
        assert_eq!(toggle("**word**", 2, 6, i), "***word***");
        assert_eq!(toggle("***word***", 3, 7, i), "**word**");
        assert_eq!(toggle("***word***", 3, 7, b), "*word*");
        assert_eq!(toggle("**word**", 1, 7, i), "***word***");

        // Separate spans are wrapped, not broken apart.
        assert_eq!(toggle("**a** and **b**", 0, 15, b), "****a** and **b****");
        // Other emphasis nested inside does not stop italic from coming off.
        assert_eq!(toggle("*a **b** c*", 1, 10, i), "a **b** c");
    }

    #[test]
    fn wrap_selects_the_text() {
        let e = toggle_emphasis(&u("a word b"), 2..6, "**");
        assert_eq!(e.selection, 4..8);
    }

    #[test]
    fn toggles_emphasis_at_the_caret() {
        // Ctrl+B, type, Ctrl+B ends the bold instead of stacking markers.
        let mut t = Typist::new("a ", 2);
        t.bold();
        t.type_("hi");
        t.bold();
        t.type_(" more");
        assert_eq!(t.text, "a **hi** more");

        // Just past the closing marker, Ctrl+B steps back into the bold.
        let mut t = Typist::new("a **hi**", 8);
        t.bold();
        t.type_("!");
        assert_eq!(t.text, "a **hi!**");

        // Inside the bold, Ctrl+B takes it off and keeps the caret in place.
        let mut t = Typist::new("a **hello** b", 6);
        t.bold();
        assert_eq!((t.text.as_str(), t.caret), ("a hello b", 4));

        // At the end of bold italic text, Ctrl+B steps out rather than unbolding.
        let mut t = Typist::new("***hello***", 8);
        t.bold();
        assert_eq!((t.text.as_str(), t.caret), ("***hello***", 11));

        // Through several layers of nesting too.
        let mut t = Typist::new("**~~*hello*~~**", 10);
        t.bold();
        assert_eq!((t.text.as_str(), t.caret), ("**~~*hello*~~**", 15));

        // A literal underscore at the end is text, not a closing marker.
        let mut t = Typist::new("**value_**", 7);
        t.bold();
        assert_eq!(t.text, "value_");

        // Italic works the same way.
        let mut t = Typist::new("a ", 2);
        t.key("*");
        t.type_("hi");
        t.key("*");
        t.type_(" x");
        assert_eq!(t.text, "a *hi* x");
    }

    #[test]
    fn no_emphasis_span_inside_a_fence() {
        // Fenced code is literal: the caret toggle wraps rather than unwrapping.
        assert_eq!(toggle("```\n**a**\n```", 6, 6, "**"), "```\n******a**\n```");
    }

    #[test]
    fn inserts_links() {
        let link = |text: &str, sel: Range, clip: &str| {
            let e = insert_link(&u(text), sel, clip);
            (apply(text, &e), e.selection)
        };
        assert_eq!(
            link("see site now", 4..8, "https://x.org"),
            ("see [site](https://x.org) now".into(), 5..9)
        );
        assert_eq!(
            link("see ", 4..4, "www.x.org"),
            ("see [](https://www.x.org)".into(), 5..5)
        );
        assert_eq!(
            link("see site now", 4..8, "not a url"),
            ("see [site]() now".into(), 11..11)
        );
        assert_eq!(link("", 0..0, "nope"), ("[]()".into(), 1..1));
        // Escapes.
        assert_eq!(
            link("a[b]", 0..4, "https://x.org/a(b)"),
            ("[a\\[b\\]](https://x.org/a\\(b\\))".into(), 1..7)
        );
    }

    #[test]
    fn never_panics_and_stays_in_range() {
        let alphabet: Vec<u16> = "a*~\n`[] _".encode_utf16().collect();
        let mut texts: Vec<Vec<u16>> = vec![vec![]];
        let mut last = texts.clone();
        for _ in 0..4 {
            last = last
                .iter()
                .flat_map(|t| alphabet.iter().map(move |&c| [t.clone(), vec![c]].concat()))
                .collect();
            texts.extend(last.clone());
        }
        for t in &texts {
            for from in 0..=t.len() + 1 {
                for to in from..=t.len() + 1 {
                    for m in ["**", "*", "~~"] {
                        let e = toggle_emphasis(t, from..to, m);
                        assert!(e.range.end <= t.len() && e.range.start <= e.range.end);
                        let after = t.len() - e.range.len() + e.replacement.encode_utf16().count();
                        assert!(
                            e.selection.start <= e.selection.end && e.selection.end <= after,
                            "{t:?} {from}..{to} {m}"
                        );
                    }
                    let e = insert_link(t, from..to, "https://x.org");
                    assert!(e.range.end <= t.len());
                }
            }
        }
    }

    #[test]
    fn out_of_range_selection_clamps() {
        let _ = toggle_emphasis(&u("abc"), 5..9, "**");
        let _ = insert_link(&u("abc"), 9..2, "https://x.org");
    }
}
