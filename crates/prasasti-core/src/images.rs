//! Inline Markdown image destinations and the path rewrite done on Save and
//! Save As. Pure UTF-16, never panics.

use crate::edit::Edit;
use crate::markdown::{style_paragraph, BlockState, StyleKind};
use crate::Range;

const SLASH: u16 = b'/' as u16;

/// One inline image destination. `range` covers the token, `<…>` included when
/// `angled`; `path` is the unescaped destination, percent-decoded for `file://`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageDest {
    pub range: Range,
    pub path: Vec<u16>,
    pub angled: bool,
}

fn u(c: char) -> u16 {
    c as u16
}

fn is_ws(c: u16) -> bool {
    matches!(c, 0x20 | 0x09 | 0x0a | 0x0d)
}

fn is_blank(c: u16) -> bool {
    matches!(c, 0x20 | 0x09)
}

/// True for every unit inside a `Code` or `Fence` span.
/// shortcut: indented code blocks are not detected (`style_paragraph` has no
/// such block), so an image in one is still rewritten; upgrade if it matters.
fn code_mask(text: &[u16]) -> Vec<bool> {
    let mut mask = vec![false; text.len()];
    let mut state = BlockState::Prose;
    let mut start = 0;
    while start <= text.len() {
        let end = text[start..].iter().position(|&c| matches!(c, 0x0a | 0x0d | 0x2029)).map_or(text.len(), |k| start + k);
        let (spans, next) = style_paragraph(&text[start..end], state, None);
        state = next;
        for s in spans.iter().filter(|s| matches!(s.kind, StyleKind::Code | StyleKind::Fence)) {
            if let Some(m) = mask.get_mut(start + s.range.start..start + s.range.end) {
                m.fill(true);
            }
        }
        start = if text.get(end) == Some(&0x0d) && text.get(end + 1) == Some(&0x0a) { end + 2 } else { end + 1 };
    }
    mask
}

fn unescape(t: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        if t[i] == u('\\') && t.get(i + 1).is_some_and(|&n| n < 128 && (n as u8).is_ascii_punctuation()) {
            i += 1;
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

/// `![alt](dest "title")` starting at `i`: the destination and the index after `)`.
/// An unwrapped destination may hold spaces, as pasted paths do; it ends at a
/// quote after a space, an unbalanced `)` or the end of the line.
fn parse_image(t: &[u16], i: usize) -> Option<(ImageDest, usize)> {
    let at = |k: usize| t.get(k).copied();
    // Alt text, with nested brackets.
    let (mut j, mut depth) = (i + 2, 1);
    while depth > 0 {
        match at(j)? {
            0x5c => j += 1,
            0x5b => depth += 1,
            0x5d => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    if at(j)? != u('(') {
        return None;
    }
    let mut p = j + 1;
    while at(p).is_some_and(is_ws) {
        p += 1;
    }
    let (q, angled) = if at(p)? == u('<') {
        let mut q = p + 1;
        loop {
            match at(q)? {
                0x5c => q += 1,
                0x0a | 0x0d | 0x3c => return None,
                0x3e => break,
                _ => {}
            }
            q += 1;
        }
        (q + 1, true)
    } else {
        let (mut q, mut depth) = (p, 0);
        while let Some(c) = at(q) {
            match c {
                0x0a | 0x0d => break,
                0x5c => q += 1,
                0x28 => depth += 1,
                0x29 if depth == 0 => break,
                0x29 => depth -= 1,
                0x22 | 0x27 if q > p && is_blank(t[q - 1]) => break,
                _ => {}
            }
            q += 1;
        }
        let mut q = q.min(t.len());
        while q > p && is_blank(t[q - 1]) {
            q -= 1;
        }
        (q, false)
    };
    if q == p {
        return None;
    }
    let mut r = q;
    while at(r).is_some_and(is_ws) {
        r += 1;
    }
    if at(r)? != u(')') {
        // A title needs a space before it; `(…)` titles only follow `<…>`.
        let close = match at(r)? {
            0x22 => 0x22,
            0x27 => 0x27,
            0x28 if angled => 0x29,
            _ => return None,
        };
        if r == q {
            return None;
        }
        r += 1;
        while at(r)? != close {
            r += (at(r)? == u('\\')) as usize + 1;
        }
        r += 1;
        while at(r).is_some_and(is_ws) {
            r += 1;
        }
        if at(r)? != u(')') {
            return None;
        }
    }
    let inner = if angled { &t[p + 1..q - 1] } else { &t[p..q] };
    Some((ImageDest { range: p..q, path: unescape(inner), angled }, r + 1))
}

/// Inline images outside code spans and fences, in text order.
pub fn image_destinations(text: &[u16]) -> Vec<ImageDest> {
    let mask = code_mask(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < text.len() {
        let escaped = text[..i].iter().rev().take_while(|&&c| c == u('\\')).count() % 2 == 1;
        if text[i] == u('!')
            && text[i + 1] == u('[')
            && !mask[i]
            && !escaped
            && let Some((d, end)) = parse_image(text, i)
        {
            out.push(d);
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

fn components(p: &[u16]) -> Vec<&[u16]> {
    let mut out: Vec<&[u16]> = Vec::new();
    for c in p.split(|&c| c == SLASH) {
        match c {
            [] | [0x2e] => {}
            [0x2e, 0x2e] => {
                out.pop();
            }
            _ => out.push(c),
        }
    }
    out
}

/// Shortest relative path from directory `from_dir` to `to`, both absolute.
pub fn relative_path(from_dir: &[u16], to: &[u16]) -> Vec<u16> {
    let (from, to) = (components(from_dir), components(to));
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&[u16]> = vec![&[0x2e, 0x2e]; from.len() - common];
    parts.extend(&to[common..]);
    if parts.is_empty() {
        return vec![0x2e];
    }
    parts.join(&SLASH)
}

/// Wraps in `<…>` when `angled` or when the path has a space, `(` or `)`.
pub fn format_destination(path: &[u16], angled: bool) -> Vec<u16> {
    if !angled && !path.iter().any(|&c| matches!(c, 0x20 | 0x28 | 0x29)) {
        return path.to_vec();
    }
    [&[u('<')][..], path, &[u('>')]].concat()
}

/// `%XX`-decoded `t`; `None` for a bad escape or invalid UTF-8.
fn percent_decode(t: &[u16]) -> Option<Vec<u16>> {
    let mut bytes = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        if t[i] == u('%') {
            let hex = |k: usize| char::from_u32(*t.get(k)? as u32)?.to_digit(16);
            bytes.push((hex(i + 1)? * 16 + hex(i + 2)?) as u8);
            i += 3;
        } else {
            let mut b = [0; 4];
            bytes.extend(char::decode_utf16([t[i]]).next()?.ok()?.encode_utf8(&mut b).as_bytes());
            i += 1;
        }
    }
    Some(String::from_utf8(bytes).ok()?.encode_utf16().collect())
}

/// Starts like `scheme:`.
fn has_scheme(p: &[u16]) -> bool {
    let n = p.iter().take_while(|&&c| c < 128 && ((c as u8).is_ascii_alphanumeric() || b"+.-".contains(&(c as u8)))).count();
    n > 0 && p.get(n) == Some(&u(':')) && (p[0] as u8).is_ascii_alphabetic()
}

fn starts_with(p: &[u16], s: &str) -> bool {
    p.len() >= s.len() && p.iter().zip(s.encode_utf16()).all(|(&a, b)| a == b)
}

/// The absolute path of a `/…` or `file://` destination.
fn absolute(p: &[u16]) -> Option<Vec<u16>> {
    if starts_with(p, "file://") {
        let rest = &p[7..];
        let rest = if starts_with(rest, "localhost/") { &rest[9..] } else { rest };
        return (rest.first() == Some(&SLASH)).then(|| percent_decode(rest)).flatten();
    }
    (p.first() == Some(&SLASH) && p.get(1) != Some(&SLASH)).then(|| p.to_vec())
}

/// Rewrites image destinations for a document now in `new_dir`.
/// `remap` may swap an absolute path before it is made relative.
pub fn rebase(
    text: &[u16],
    old_dir: Option<&[u16]>,
    new_dir: &[u16],
    remap: impl Fn(&[u16]) -> Option<Vec<u16>>,
) -> Option<Edit> {
    let mut changes: Vec<(Range, Vec<u16>)> = Vec::new();
    for d in image_destinations(text) {
        let p = &d.path;
        let new = if let Some(abs) = absolute(p) {
            let abs = remap(&abs).unwrap_or(abs);
            relative_path(new_dir, &abs)
        } else if p.is_empty() || p[0] == u('#') || has_scheme(p) || starts_with(p, "//") {
            continue;
        } else if let Some(old) = old_dir.filter(|&o| components(o) != components(new_dir)) {
            relative_path(new_dir, &[old, &[SLASH], p].concat())
        } else {
            continue;
        };
        // Characters that would need escaping in the written form: keep the original.
        if new.iter().any(|&c| matches!(c, 0x5c | 0x3c | 0x3e | 0x0a | 0x0d)) {
            continue;
        }
        let token = format_destination(&new, d.angled);
        if token != text[d.range.clone()] {
            changes.push((d.range, token));
        }
    }
    let start = changes.first()?.0.start;
    let end = changes.last()?.0.end;
    let mut out = Vec::new();
    let mut at = start;
    for (r, token) in &changes {
        out.extend_from_slice(&text[at..r.start]);
        out.extend_from_slice(token);
        at = r.end;
    }
    // A lone surrogate between destinations cannot go through a `String`: leave the text alone.
    let replacement = String::from_utf16(&out).ok()?;
    let caret = start + out.len();
    Some(Edit { range: start..end, replacement, selection: caret..caret })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn apply(text: &str, e: &Edit) -> String {
        let mut t = w(text);
        t.splice(e.range.clone(), e.replacement.encode_utf16());
        String::from_utf16(&t).unwrap()
    }

    fn rb_with(text: &str, old: Option<&str>, new: &str, remap: impl Fn(&[u16]) -> Option<Vec<u16>>) -> Option<String> {
        let old = old.map(w);
        rebase(&w(text), old.as_deref(), &w(new), remap).map(|e| apply(text, &e))
    }

    fn rb(text: &str, old: Option<&str>, new: &str) -> Option<String> {
        rb_with(text, old, new, |_| None)
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn abs_same_dir() {
        assert_eq!(rb("![a](/d/img.png)", None, "/d"), some("![a](img.png)"));
    }

    #[test]
    fn abs_subdir() {
        assert_eq!(rb("![a](/d/x/img.png \"t\")", None, "/d"), some("![a](x/img.png \"t\")"));
    }

    #[test]
    fn abs_parent() {
        assert_eq!(rb("![a](/s/img.png)", None, "/d/n"), some("![a](../../s/img.png)"));
    }

    #[test]
    fn file_url() {
        assert_eq!(rb("![a](file:///d/a%20b.png)", None, "/d"), some("![a](<a b.png>)"));
    }

    #[test]
    fn rel_save_as() {
        assert_eq!(rb("![a](img.png)", Some("/old"), "/new"), some("![a](../old/img.png)"));
    }

    #[test]
    fn rel_same_dir_kept() {
        assert_eq!(rb("![a](img.png)", Some("/d"), "/d"), None);
    }

    #[test]
    fn rel_untitled_kept() {
        assert_eq!(rb("![a](img.png)", None, "/d"), None);
    }

    #[test]
    fn urls_kept() {
        assert_eq!(rb("![a](https://x/y.png) ![b](data:x) ![c](#f)", None, "/d"), None);
    }

    #[test]
    fn code_kept() {
        assert_eq!(rb("`![a](/d/i.png)`", None, "/d"), None);
        assert_eq!(rb("```\n![a](/d/i.png)\n```\n", None, "/d"), None);
        assert_eq!(rb("~~~\r\n![a](/d/i.png)\r\n~~~", None, "/d"), None);
    }

    #[test]
    fn after_fence_rewritten() {
        assert_eq!(rb("```\nx\n```\n![a](/d/i.png)", None, "/d"), some("```\nx\n```\n![a](i.png)"));
    }

    #[test]
    fn angled_kept() {
        assert_eq!(rb("![a](</d/i.png>)", None, "/d"), some("![a](<i.png>)"));
    }

    #[test]
    fn non_ascii() {
        assert_eq!(rb("![a](/d/ü 图.png)", None, "/d"), some("![a](<ü 图.png>)"));
        assert_eq!(rb("![a](</d/ü 图.png> 'x')", None, "/d"), some("![a](<ü 图.png> 'x')"));
        assert_eq!(rb("![a](file:///d/%C3%BC.png)", None, "/d"), some("![a](ü.png)"));
    }

    #[test]
    fn remap_used() {
        let r = rb_with("![a](/p/i.png)", None, "/d", |p| (p == w("/p/i.png")).then(|| w("/d/x.assets/i.png")));
        assert_eq!(r, some("![a](x.assets/i.png)"));
    }

    #[test]
    fn two_images_one_edit() {
        let text = "pre ![a](/d/1.png) text ![b](/d/2.png) post";
        let e = rebase(&w(text), None, &w("/d"), |_| None).unwrap();
        assert_eq!(e.range.start, text.find("/d/1").unwrap());
        assert_eq!(apply(text, &e), "pre ![a](1.png) text ![b](2.png) post");
        assert_eq!(e.selection.start, e.range.start + e.replacement.encode_utf16().count());
    }

    #[test]
    fn spaces_and_parens() {
        assert_eq!(rb("![a](</d/a b (1).png>)", None, "/d/x"), some("![a](<../a b (1).png>)"));
        assert_eq!(rb("![a](/d/a(1).png)", None, "/d"), some("![a](<a(1).png>)"));
        assert_eq!(rb("![a](/d/a b.png \"t\")", None, "/d"), some("![a](<a b.png> \"t\")"));
    }

    #[test]
    fn rejects_non_images() {
        for s in ["[a](/d/i.png)", "![a]", "![a](", "![a]()", "![a](/d/i.png", "\\![a](/d/i.png)"] {
            assert_eq!(rb(s, None, "/d/x"), None, "{s}");
        }
    }

    #[test]
    fn relative_paths() {
        let r = |a: &str, b: &str| String::from_utf16(&relative_path(&w(a), &w(b))).unwrap();
        assert_eq!(r("/a/b", "/a/b/c.png"), "c.png");
        assert_eq!(r("/a/b/", "/a/c/d.png"), "../c/d.png");
        assert_eq!(r("/a/b", "/a/b"), ".");
        assert_eq!(r("/", "/x"), "x");
        assert_eq!(r("/a/../b", "/b/x"), "x");
        assert_eq!(r("/a", "/../x"), "../x");
    }

    #[test]
    fn format() {
        let f = |s: &str, a| String::from_utf16(&format_destination(&w(s), a)).unwrap();
        assert_eq!(f("a.png", false), "a.png");
        assert_eq!(f("a.png", true), "<a.png>");
        assert_eq!(f("a b.png", false), "<a b.png>");
        assert_eq!(f("a(1).png", false), "<a(1).png>");
    }
}
