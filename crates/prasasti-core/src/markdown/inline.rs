//! Inline Markdown spans over UTF-16 text. Patterns are hand-matched so `core`
//! stays dependency-free; each matcher notes the regex it stands for.

use crate::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineKind {
    Bold,
    Italic,
    Strike,
    Link,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMarkup {
    pub kind: InlineKind,
    pub content: Range,
    pub markers: [Range; 2],
    /// Link destination as written, escapes included. `None` for non-links.
    pub destination: Option<Range>,
}

const STAR: u16 = b'*' as u16;
const UNDERSCORE: u16 = b'_' as u16;
const TILDE: u16 = b'~' as u16;
const BACKTICK: u16 = b'`' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const NEWLINE: u16 = b'\n' as u16;
const OBJECT_REPLACEMENT: u16 = 0xFFFC;

/// PCRE `\s` without the Unicode-properties option.
fn ascii_space(u: u16) -> bool {
    u == 0x20 || (0x09..=0x0D).contains(&u)
}

fn scalar(high: u16, low: u16) -> Option<char> {
    char::from_u32(0x10000 + (((high as u32) - 0xD800) << 10) + ((low as u32) - 0xDC00))
}

/// The scalar value starting at `i`, if it is one (not a lone surrogate).
fn char_at(t: &[u16], i: usize) -> Option<char> {
    let u = *t.get(i)?;
    match t.get(i + 1) {
        Some(&l) if (0xD800..0xDC00).contains(&u) && (0xDC00..0xE000).contains(&l) => scalar(u, l),
        _ => char::from_u32(u as u32),
    }
}

/// The scalar value ending at `i`.
fn char_before(t: &[u16], i: usize) -> Option<char> {
    let u = *t.get(i.checked_sub(1)?)?;
    if (0xDC00..0xE000).contains(&u) && i >= 2 && (0xD800..0xDC00).contains(&t[i - 2]) {
        return scalar(t[i - 2], u);
    }
    char::from_u32(u as u32)
}

/// PCRE `\w` with Unicode properties.
fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

fn is_space(c: Option<char>) -> bool {
    c.is_some_and(char::is_whitespace)
}

fn backslashes_before(t: &[u16], i: usize) -> usize {
    t[..i.min(t.len())].iter().rev().take_while(|&&u| u == BACKSLASH).count()
}

/// `` `([^`]+)` `` anchored at each unescaped backtick, backticks included.
pub fn code_spans(text: &[u16]) -> Vec<Range> {
    let mut spans = Vec::new();
    let mut from = 0;
    while from < text.len() {
        if text[from] != BACKTICK {
            from += 1;
            continue;
        }
        // Backslashes escape opening backticks in prose, but are literal once
        // inside code, including immediately before the closing backtick.
        if backslashes_before(text, from) % 2 != 0 {
            from += 1;
            continue;
        }
        let close = text[from + 1..].iter().position(|&u| u == BACKTICK).map(|p| from + 1 + p);
        match close {
            Some(close) if close > from + 1 => {
                spans.push(from..close + 1);
                from = close + 1;
            }
            _ => from += 1,
        }
    }
    spans
}

/// `^\s{0,3}([-*_])(?:\s*\1){2,}\s*$`
fn is_thematic_break(t: &[u16]) -> bool {
    let lead = t.iter().take(3).take_while(|&&u| ascii_space(u)).count();
    let Some(&marker) = t.get(lead) else { return false };
    if ![b'-' as u16, STAR, UNDERSCORE].contains(&marker) {
        return false;
    }
    let rest = &t[lead + 1..];
    rest.iter().all(|&u| u == marker || ascii_space(u)) && rest.iter().filter(|&&u| u == marker).count() >= 2
}

/// Every non-overlapping match from `at`, scanning left to right as a global
/// regex match does. `at` returns the whole match and what it captured.
fn global<T>(len: usize, mut at: impl FnMut(usize) -> Option<(Range, T)>) -> Vec<(Range, T)> {
    let mut found = Vec::new();
    let mut s = 0;
    while s < len {
        match at(s) {
            Some((whole, captured)) => {
                s = whole.end.max(s + 1);
                found.push((whole, captured));
            }
            None => s += 1,
        }
    }
    found
}

/// `(?<!d)d{n}(?![\s d])(.*?[^\s d])d{n}(?!d)` at `s`, for `d` = `*` (n = 3) or
/// `~` (n = 2). Returns the group's end.
fn run_at(t: &[u16], s: usize, d: u16, n: usize) -> Option<usize> {
    let is_d = |i: usize| t.get(i) == Some(&d);
    if (s > 0 && is_d(s - 1)) || !(0..n).all(|k| is_d(s + k)) {
        return None;
    }
    let first = *t.get(s + n)?;
    if ascii_space(first) || first == d {
        return None;
    }
    for g in s + n + 1..=t.len().saturating_sub(n) {
        let last = t[g - 1];
        if last == NEWLINE {
            break;
        }
        if !ascii_space(last) && last != d && (0..n).all(|k| is_d(g + k)) && !is_d(g + n) {
            return Some(g);
        }
    }
    None
}

/// `\*\*(.+?)\*\*` or `(?<!\w)__(.+?)__(?!\w)` at `s`.
fn bold_at(t: &[u16], s: usize) -> Option<(Range, Range)> {
    let pair = |i: usize, d: u16| t.get(i) == Some(&d) && t.get(i + 1) == Some(&d);
    for d in [STAR, UNDERSCORE] {
        if !pair(s, d) || (d == UNDERSCORE && is_word(char_before(t, s))) {
            continue;
        }
        for g in s + 3..=t.len().saturating_sub(2) {
            if t[g - 1] == NEWLINE {
                break;
            }
            if pair(g, d) && (d == STAR || !is_word(char_at(t, g + 2))) {
                return Some((s..g + 2, s + 2..g));
            }
        }
    }
    None
}

/// `(?<!\*)\*(?![\s*])([^*\n]*[^\s*])\*(?!\*)` or `(?<!\w)_([^_\n]+)_(?!\w)`
/// at `s`. Both groups end at the first delimiter or newline, so no search.
fn italic_at(t: &[u16], s: usize) -> Option<(Range, Range)> {
    let d = *t.get(s)?;
    if d != STAR && d != UNDERSCORE {
        return None;
    }
    let r = (s + 1..t.len()).find(|&i| t[i] == d || t[i] == NEWLINE)?;
    if t[r] != d || r == s + 1 {
        return None;
    }
    if d == STAR {
        let ok = (s == 0 || t[s - 1] != STAR)
            && !is_space(char_at(t, s + 1))
            && r >= s + 2
            && !is_space(char_before(t, r))
            && t.get(r + 1) != Some(&STAR);
        ok.then_some((s..r + 1, s + 1..r))
    } else {
        let ok = !is_word(char_before(t, s)) && !is_word(char_at(t, r + 1));
        ok.then_some((s..r + 1, s + 1..r))
    }
}

/// End of `(?:\\.|\([^()]*\)|[^()])+` at `i` followed by `\)`, with the
/// backtracking a regex engine does: the first successful path in priority
/// order wins. Returns the index of the closing parenthesis.
fn destination_end(t: &[u16], start: usize) -> Option<usize> {
    let mut dead = vec![false; t.len() + 1];
    // (position, next alternative to try); alternative 3 closes the group.
    let mut stack = vec![(start, 0u8)];
    while let Some(frame) = stack.last_mut() {
        let (pos, alt) = *frame;
        frame.1 += 1;
        let u = t.get(pos).copied();
        let next = match alt {
            0 => match (u, t.get(pos + 1)) {
                (Some(BACKSLASH), Some(&c)) if c != NEWLINE => Some(pos + 2),
                _ => None,
            },
            1 if u == Some(b'(' as u16) => {
                let close = (pos + 1..t.len()).find(|&i| t[i] == b'(' as u16 || t[i] == b')' as u16);
                close.filter(|&i| t[i] == b')' as u16).map(|i| i + 1)
            }
            2 if u.is_some_and(|u| u != b'(' as u16 && u != b')' as u16) => Some(pos + 1),
            3 if stack.len() > 1 && u == Some(b')' as u16) => return Some(pos),
            4.. => {
                dead[pos] = true;
                stack.pop();
                None
            }
            _ => None,
        };
        if let Some(next) = next.filter(|&n| !dead[n]) {
            stack.push((next, 0));
        }
    }
    None
}

/// `\[([^\]]+)\]\(((?:\\.|\([^()]*\)|[^()])+)\)` at `s`: whole, content, destination.
fn link_at(t: &[u16], s: usize) -> Option<(Range, Range, Range)> {
    if t.get(s) != Some(&(b'[' as u16)) {
        return None;
    }
    let r = (s + 1..t.len()).find(|&i| t[i] == b']' as u16)?;
    if r == s + 1 || t.get(r + 1) != Some(&(b'(' as u16)) {
        return None;
    }
    let end = destination_end(t, r + 2)?;
    Some((s..end + 1, s + 1..r, r + 2..end))
}

pub fn inline_markup(text: &[u16]) -> Vec<InlineMarkup> {
    let mut markup = Vec::new();
    if !text.iter().any(|u| [STAR, UNDERSCORE, b'[' as u16, TILDE].contains(u)) || is_thematic_break(text) {
        return markup;
    }

    // Inline code is literal, so a marker inside backticks is not a marker:
    // `default_line_height` keeps its underscores. Markup whose markers land in
    // a code span is dropped; markup that merely wraps one still applies.
    // A backslash escapes a marker, as it does a backtick.
    let code = code_spans(text);
    let append = |markup: &mut Vec<InlineMarkup>, item: InlineMarkup| {
        let blocked = item.markers.iter().any(|m| {
            backslashes_before(text, m.start) % 2 != 0 || code.iter().any(|c| m.start < c.end && c.start < m.end)
        });
        if !blocked {
            markup.push(item);
        }
        !blocked
    };
    let item = |kind, content, markers, destination| InlineMarkup { kind, content, markers, destination };

    // "***word***" is bold and italic at once: the outer two asterisks of each
    // run are bold, the inner one italic. Its runs are then masked so the
    // patterns below do not split them again.
    let mut unclaimed = text.to_vec();
    for (whole, group) in global(text.len(), |s| {
        let g = run_at(text, s, STAR, 3)?;
        Some((s..g + 3, s + 3..g))
    }) {
        let bold = item(
            InlineKind::Bold,
            whole.start + 2..whole.end - 2,
            [whole.start..whole.start + 2, whole.end - 2..whole.end],
            None,
        );
        if !append(&mut markup, bold) {
            continue;
        }
        markup.push(item(
            InlineKind::Italic,
            group,
            [whole.start + 2..whole.start + 3, whole.end - 3..whole.end - 2],
            None,
        ));
        for i in [0, 1, 2].into_iter().flat_map(|k| [whole.start + k, whole.end - 1 - k]) {
            unclaimed[i] = OBJECT_REPLACEMENT;
        }
    }

    // Underscores only delimit emphasis at a word boundary; asterisks anywhere.
    for (whole, content) in global(unclaimed.len(), |s| bold_at(&unclaimed, s)) {
        let markers = [whole.start..whole.start + 2, whole.end - 2..whole.end];
        append(&mut markup, item(InlineKind::Bold, content, markers, None));
    }
    for (whole, content) in global(unclaimed.len(), |s| italic_at(&unclaimed, s)) {
        let markers = [whole.start..whole.start + 1, whole.end - 1..whole.end];
        append(&mut markup, item(InlineKind::Italic, content, markers, None));
    }
    // Exactly two tildes, opening before text and closing after it.
    for (whole, content) in global(unclaimed.len(), |s| {
        let g = run_at(&unclaimed, s, TILDE, 2)?;
        Some((s..g + 2, s + 2..g))
    }) {
        let markers = [whole.start..whole.start + 2, whole.end - 2..whole.end];
        append(&mut markup, item(InlineKind::Strike, content, markers, None));
    }
    for (whole, (content, destination)) in global(unclaimed.len(), |s| {
        let (whole, content, destination) = link_at(&unclaimed, s)?;
        Some((whole, (content, destination)))
    }) {
        let markers = [whole.start..whole.start + 1, content.end..whole.end];
        append(&mut markup, item(InlineKind::Link, content, markers, Some(destination)));
    }
    markup
}

/// The URL of the link whose text holds `pos`, with Markdown escapes removed.
pub fn link_url_at(text: &[u16], pos: usize) -> Option<String> {
    if pos >= text.len() {
        return None;
    }
    let link = inline_markup(text)
        .into_iter()
        .find(|m| m.kind == InlineKind::Link && m.content.contains(&pos))?;
    let dest = &text[link.destination?];
    let mut url = Vec::with_capacity(dest.len());
    let mut i = 0;
    while i < dest.len() {
        // `\` before ASCII punctuation is an escape: [!-/:-@\[-`{-~].
        let punctuation = |u: u16| u < 0x80 && (u as u8).is_ascii_punctuation();
        if dest[i] == BACKSLASH && dest.get(i + 1).is_some_and(|&u| punctuation(u)) {
            i += 1;
        }
        url.push(dest[i]);
        i += 1;
    }
    Some(String::from_utf16_lossy(&url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn markup(s: &str) -> Vec<InlineMarkup> {
        inline_markup(&u(s))
    }

    fn url(s: &str, pos: usize) -> Option<String> {
        link_url_at(&u(s), pos)
    }

    fn slice(s: &str, r: &Range) -> String {
        String::from_utf16_lossy(&u(s)[r.clone()])
    }

    #[test]
    fn finds_inline_markdown_ranges() {
        let m = markup("**bold** and *italic* and [site](https://example.com)");
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].content, 2..6);
        assert_eq!(m[2].content.len(), 4);
        assert_eq!(m[2].markers[0].len(), 1);
    }

    #[test]
    fn finds_link_url_at_position() {
        let text = "Read [site](https://example.com/docs) now";
        let start = text.find("site").unwrap();
        assert_eq!(url(text, start).as_deref(), Some("https://example.com/docs"));
        assert_eq!(url(text, start + 3).as_deref(), Some("https://example.com/docs"));
        assert_eq!(url(text, start - 1), None);
        assert_eq!(url(text, text.find("https").unwrap()), None);

        // Escaped punctuation, as the editor writes for parentheses, reads as itself.
        assert_eq!(url(r"[wiki](https://example.com/a\(b\))", 1).as_deref(), Some("https://example.com/a(b)"));
    }

    #[test]
    fn finds_strikethrough_outside_code() {
        let m = markup("a ~~gone~~ b");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].kind, InlineKind::Strike);
        assert_eq!(m[0].content, 4..8);

        // Tildes in code, beside space, or in longer runs stay literal.
        assert!(markup("`~~x~~`").is_empty());
        assert!(markup("a ~~ b ~~ c").is_empty());
        assert!(markup("~~~x~~~").is_empty());
    }

    #[test]
    fn leaves_inline_code_literal() {
        assert!(markup("see `default_line_height` and `file_name`").is_empty());
        assert!(markup("a_b `c_d`").is_empty());
        // Word-boundary underscores inside a code span are still literal.
        assert!(markup("`_private_`").is_empty());
        assert!(markup("`a * b * c`").is_empty());
        assert!(markup("`[not](a link)`").is_empty());

        // Emphasis that merely wraps a code span still applies.
        let wrapping = markup("**`file_name` only**");
        assert_eq!(wrapping.len(), 1);
        assert_eq!(wrapping[0].kind, InlineKind::Bold);
        assert_eq!(wrapping[0].content.start, 2);
        assert_eq!(wrapping[0].content.len(), 16);
        assert_eq!(wrapping[0].markers[0].len(), 2);
        assert_eq!(wrapping[0].markers[1].start, 18);
    }

    #[test]
    fn keeps_intraword_underscores_literal() {
        assert!(markup("snake_case_name and default_line_height").is_empty());
        assert!(markup("a__b__c").is_empty());

        // Emphasis still opens at a word boundary.
        let bold = markup("say __bold__ here");
        assert_eq!(bold.len(), 1);
        assert_eq!(bold[0].content, 6..10);

        let emphasis = markup("_italic_, **bold** and mid*word*");
        assert_eq!(emphasis.len(), 3);
        assert_eq!(emphasis[0].kind, InlineKind::Bold);
        assert_eq!(emphasis[0].content.len(), 4);
        assert_eq!(emphasis[1].content.start, 1);
        assert_eq!(emphasis[1].content.len(), 6);
        assert_eq!(emphasis[2].content.len(), 4);
    }

    #[test]
    fn respects_escaped_opening_backticks() {
        let escaped = markup(r"\` *italic* \`");
        assert_eq!(escaped.len(), 1);
        assert_eq!(escaped[0].kind, InlineKind::Italic);
        assert!(code_spans(&u(r"\` *italic* \`")).is_empty());

        let mixed = r"\` *italic* `*literal*`";
        assert_eq!(markup(mixed).len(), 1);
        let code = code_spans(&u(mixed));
        assert_eq!(code.len(), 1);
        assert_eq!(slice(mixed, &code[0]), "`*literal*`");

        // An escaped backslash leaves the opening backtick active.
        assert!(markup(r"\\`*literal*`").is_empty());
        assert_eq!(code_spans(&u(r"\\`*literal*`")).len(), 1);

        // Backslashes inside code do not escape its closing delimiter.
        let after = markup(r"`code\` *italic*");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].kind, InlineKind::Italic);
    }

    #[test]
    fn honours_backslash_escapes() {
        assert!(markup("\\*not\\* here").is_empty());
        assert!(markup("\\[x](y)").is_empty());
        assert!(markup("\\~~not~~").is_empty());
        // An escaped backslash leaves the marker alone.
        assert_eq!(markup("\\\\*yes*").len(), 1);
    }

    #[test]
    fn reads_link_destinations_with_parentheses() {
        assert_eq!(url("[a](b(c))", 1).as_deref(), Some("b(c)"));
        assert_eq!(url("[a](b) and (c)", 1).as_deref(), Some("b"));
    }

    #[test]
    fn ignores_inline_markdown_inside_code_spans() {
        assert_eq!(markup("`The_brown_fox` jumps").len(), 0);
        assert_eq!(markup("`a **b** [c](d)` and `e`").len(), 0);

        let mixed = markup("`code_span` then *real* emphasis");
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].kind, InlineKind::Italic);
        assert_eq!(mixed[0].content.start, 18);
    }

    #[test]
    fn keeps_markup_that_encloses_a_code_span() {
        let spanning = markup("_a `b` c_");
        assert_eq!(spanning.len(), 1);
        assert_eq!(spanning[0].kind, InlineKind::Italic);
        assert_eq!(spanning[0].content, 1..8);

        let linked = markup("[see `code`](url)");
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].kind, InlineKind::Link);
        assert_eq!(linked[0].content, 1..11);

        let wrapped = markup("*`code`*");
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].kind, InlineKind::Italic);

        let bold = markup("**a `b` c**");
        assert_eq!(bold.len(), 1);
        assert_eq!(bold[0].kind, InlineKind::Bold);
        assert_eq!(bold[0].content, 2..9);

        // The closing underscore is inside the code span, so it is not a marker.
        assert_eq!(markup("_a `b_ c` d_").len(), 0);
    }

    #[test]
    fn keeps_whitespace_delimited_asterisks_literal() {
        // Only `*important*` is markup; the bullet stays a bullet.
        let bullet = markup("* first *important* point");
        assert_eq!(bullet.len(), 1);
        assert_eq!(bullet[0].kind, InlineKind::Italic);
        assert_eq!(bullet[0].content.start, 9);
        assert_eq!(bullet[0].content.len(), 9);
        assert_eq!(bullet[0].markers[0].start, 8);
        assert_eq!(bullet[0].markers[1].start, 18);

        // Multiplication stays exactly as typed.
        assert_eq!(markup("2 * 3 * 4").len(), 0);

        let short = markup("*a*");
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].kind, InlineKind::Italic);
        assert_eq!(short[0].content, 1..2);

        let long = markup("*longer text*");
        assert_eq!(long.len(), 1);
        assert_eq!(long[0].content.len(), 11);

        let mixed = markup("**bold** and *italic*");
        assert_eq!(mixed.len(), 2);
        assert_eq!(mixed[0].kind, InlineKind::Bold);
        assert_eq!(mixed[1].kind, InlineKind::Italic);
    }

    #[test]
    fn ranges_are_utf16_after_emoji() {
        let m = inline_markup(&u("😀 **b**"));
        assert_eq!(m[0].content, 5..6); // 😀 is 2 units, then space and **
    }

    #[test]
    fn bold_italic_run_is_one_bold_and_one_italic() {
        let m = markup("***word***");
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].kind, m[0].content.clone()), (InlineKind::Bold, 2..8));
        assert_eq!(m[0].markers, [0..2, 8..10]);
        assert_eq!((m[1].kind, m[1].content.clone()), (InlineKind::Italic, 3..7));
        assert_eq!(m[1].markers, [2..3, 7..8]);
    }

    #[test]
    fn thematic_break_has_no_inline_markup() {
        assert!(markup("***").is_empty());
        assert!(markup("* * *").is_empty());
        assert!(markup("___").is_empty());
    }

    #[test]
    fn link_destination_is_recorded() {
        let m = markup("[a](b)");
        assert_eq!(m[0].destination, Some(4..5));
        assert_eq!(m[0].markers, [0..1, 2..6]);
        assert_eq!(markup("**x**")[0].destination, None);
    }

    #[test]
    fn link_url_past_end_is_none() {
        assert_eq!(url("[a](b)", 6), None);
        assert_eq!(url("[a](b)", 1000), None);
        assert_eq!(url("", 0), None);
    }

    #[test]
    fn never_panics_on_odd_input() {
        for s in ["[", "[a](", "[a](\\", "[a](b", "**", "*", "_", "~~", "```", "`", "\\", "[](x)", "[a](()", "**\n**"] {
            let _ = markup(s);
            let _ = code_spans(&u(s));
            let _ = url(s, 1);
        }
        // Lone surrogates.
        let _ = inline_markup(&[0xD800, STAR, 0xDC00, STAR, UNDERSCORE, 0xD83D]);
        let _ = inline_markup(&[0xDC00, UNDERSCORE, 0x61, UNDERSCORE, 0xD800]);
    }
}
