use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSFont, NSFontAttributeName, NSFontManager, NSFontTraitMask,
    NSForegroundColorAttributeName, NSStrikethroughStyleAttributeName, NSTextStorage,
    NSUnderlineStyle, NSUnderlineStyleAttributeName,
};
use objc2_foundation::{MainThreadMarker, NSDictionary, NSNumber, NSRange, NSString, ns_string};
use prasasti_core::markdown::{BlockState, StyleKind, style_paragraph};

use crate::theme::Palette;
use crate::util::to_u16;

/// Attribute (`NSNumber` true) on markers the layout manager draws as null glyphs.
pub fn hidden_key() -> &'static NSString {
    ns_string!("PrasastiHidden")
}

/// Live Markdown styling. `states[i]` is the block state paragraph `i` hands on.
#[derive(Default)]
pub struct Styler {
    pub states: Vec<BlockState>,
}

/// One character's combined style: traits add, the last colour wins.
#[derive(Clone, Copy, PartialEq, Default)]
struct Look {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    hidden: bool,
    fg: Fg,
    code_bg: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Fg {
    #[default]
    Text,
    Accent,
    Marker,
}

impl Look {
    fn add(&mut self, kind: StyleKind) {
        match kind {
            StyleKind::Heading(_) => (self.bold, self.fg) = (true, Fg::Text),
            StyleKind::Bold => self.bold = true,
            StyleKind::Italic => self.italic = true,
            StyleKind::Strike => self.strike = true,
            StyleKind::Code => (self.fg, self.code_bg) = (Fg::Text, true),
            StyleKind::Fence => (self.fg, self.code_bg) = (Fg::Marker, true),
            StyleKind::Quote => (self.fg, self.italic) = (Fg::Marker, true),
            StyleKind::Link => (self.fg, self.underline) = (Fg::Accent, true),
            StyleKind::Marker => self.fg = Fg::Marker,
            StyleKind::HiddenMarker => self.hidden = true,
        }
    }
}

/// Fonts, colours and attribute values for one restyle pass.
struct Ctx<'a> {
    storage: &'a NSTextStorage,
    pal: &'a Palette,
    base: Retained<NSDictionary<NSString, AnyObject>>,
    /// Indexed by `bold as usize | (italic as usize) << 1`.
    fonts: [Retained<NSFont>; 4],
    yes: Retained<NSNumber>,
    single: Retained<NSNumber>,
}

impl<'a> Ctx<'a> {
    fn new(storage: &'a NSTextStorage, font: &NSFont, pal: &'a Palette) -> Self {
        let fm = NSFontManager::sharedFontManager(MainThreadMarker::new().expect("main thread"));
        let bold = fm.convertFont_toHaveTrait(font, NSFontTraitMask::BoldFontMask);
        let italic = fm.convertFont_toHaveTrait(font, NSFontTraitMask::ItalicFontMask);
        let bold_italic = fm.convertFont_toHaveTrait(&bold, NSFontTraitMask::ItalicFontMask);
        let base = NSDictionary::from_slices(
            &unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] },
            &[font.as_ref(), pal.text.as_ref()],
        );
        Ctx {
            storage,
            pal,
            base,
            fonts: [font.retain(), bold, italic, bold_italic],
            yes: NSNumber::new_bool(true),
            single: NSNumber::new_isize(NSUnderlineStyle::Single.0),
        }
    }

    /// Replaces all attributes in `r` with the base font and text colour.
    fn reset(&self, r: NSRange) {
        // SAFETY: every value matches its attribute key's documented type.
        unsafe { self.storage.setAttributes_range(Some(&self.base), r) };
    }

    /// Restyles one paragraph and returns the state it hands on.
    fn paragraph(&self, buf: &[u16], p: Para, prev: BlockState, caret: usize, reset: bool) -> BlockState {
        let text = &buf[p.start..p.content_end];
        let rel = (p.start..=p.content_end).contains(&caret).then(|| caret - p.start);
        let (spans, next) = style_paragraph(text, prev, rel);
        if reset {
            self.reset(NSRange::new(p.start, p.end - p.start));
        }
        if spans.is_empty() {
            return next;
        }
        let mut looks = vec![Look::default(); text.len()];
        for sp in &spans {
            for l in &mut looks[sp.range.clone()] {
                l.add(sp.kind);
            }
        }
        let mut i = 0;
        while i < looks.len() {
            let l = looks[i];
            let j = looks[i..].iter().position(|x| *x != l).map_or(looks.len(), |k| i + k);
            if l != Look::default() {
                self.apply(l, NSRange::new(p.start + i, j - i));
            }
            i = j;
        }
        next
    }

    fn apply(&self, l: Look, r: NSRange) {
        let s = self.storage;
        let pal = self.pal;
        // SAFETY: every value matches its attribute key's documented type.
        unsafe {
            if l.bold || l.italic {
                s.addAttribute_value_range(NSFontAttributeName, &self.fonts[l.bold as usize | (l.italic as usize) << 1], r);
            }
            match l.fg {
                Fg::Text => {}
                Fg::Accent => s.addAttribute_value_range(NSForegroundColorAttributeName, &pal.accent, r),
                Fg::Marker => s.addAttribute_value_range(NSForegroundColorAttributeName, &pal.marker, r),
            }
            if l.code_bg {
                s.addAttribute_value_range(NSBackgroundColorAttributeName, &pal.code_bg, r);
            }
            if l.strike {
                s.addAttribute_value_range(NSStrikethroughStyleAttributeName, &self.single, r);
            }
            if l.underline {
                s.addAttribute_value_range(NSUnderlineStyleAttributeName, &self.single, r);
            }
            if l.hidden {
                s.addAttribute_value_range(hidden_key(), &self.yes, r);
            }
        }
    }
}

/// A paragraph: `start..content_end` is its text, `content_end..end` its separator.
#[derive(Clone, Copy)]
struct Para {
    start: usize,
    content_end: usize,
    end: usize,
}

// The paragraph separators of `NSString paragraphRangeForRange:`: LF, CR, CRLF, U+2029.
// shortcut: own scan instead of one paragraphRangeForRange: call per paragraph, so
// paragraph counting and enumeration agree and stay cheap on big documents.
fn is_sep(u: u16) -> bool {
    u == 0x0a || u == 0x0d || u == 0x2029
}

/// The paragraph that starts at `start`.
fn para_at(buf: &[u16], start: usize) -> Para {
    let content_end = buf[start..].iter().position(|&u| is_sep(u)).map_or(buf.len(), |k| start + k);
    let end = match buf.get(content_end) {
        None => content_end,
        Some(0x0d) if buf.get(content_end + 1) == Some(&0x0a) => content_end + 2,
        Some(_) => content_end + 1,
    };
    Para { start, content_end, end }
}

/// Index and start of the paragraph holding `pos` (clamped to the text).
fn locate(buf: &[u16], pos: usize) -> (usize, usize) {
    let pos = pos.min(buf.len());
    let mut start = buf[..pos].iter().rposition(|&u| is_sep(u)).map_or(0, |k| k + 1);
    // `pos` between CR and LF belongs to the paragraph the CRLF ends.
    if start == pos && pos > 0 && buf[pos - 1] == 0x0d && buf.get(pos) == Some(&0x0a) {
        start = buf[..pos - 1].iter().rposition(|&u| is_sep(u)).map_or(0, |k| k + 1);
    }
    (count_paras(&buf[..start]) - 1, start)
}

/// Paragraph count: separators (CRLF counts once) plus one.
fn count_paras(buf: &[u16]) -> usize {
    let crlf = buf.windows(2).filter(|w| w == &[0x0d, 0x0a]).count();
    buf.iter().filter(|&&u| is_sep(u)).count() - crlf + 1
}

impl Styler {
    fn prev(&self, i: usize) -> BlockState {
        if i == 0 { BlockState::Prose } else { self.states[i - 1] }
    }

    pub fn restyle_all(&mut self, storage: &NSTextStorage, base: &NSFont, pal: &Palette, caret: usize) {
        let buf = to_u16(&storage.string());
        let ctx = Ctx::new(storage, base, pal);
        self.states.clear();
        storage.beginEditing();
        // One reset up front, then styling front to back: the unstyled rest stays a
        // single attribute run, so each change is cheap. Resetting per paragraph
        // reshuffles the storage's run array every time (6 s on 1.2M units).
        ctx.reset(NSRange::new(0, buf.len()));
        let mut start = 0;
        loop {
            let p = para_at(&buf, start);
            let next = ctx.paragraph(&buf, p, self.prev(self.states.len()), caret, false);
            self.states.push(next);
            if p.end == p.content_end {
                break;
            }
            start = p.end;
        }
        storage.endEditing();
    }

    /// Restyles the paragraphs `edited` touches, then on while a paragraph's
    /// state differs from the cache. Returns the range restyled.
    pub fn restyle_edit(&mut self, storage: &NSTextStorage, base: &NSFont, pal: &Palette, edited: NSRange, caret: usize) -> NSRange {
        let buf = to_u16(&storage.string());
        let total = count_paras(&buf);
        if self.states.is_empty() {
            self.restyle_all(storage, base, pal, caret);
            return NSRange::new(0, buf.len());
        }
        let (first, start) = locate(&buf, edited.location);
        let (last, _) = locate(&buf, edited.location + edited.length);
        // Paragraphs appeared or merged inside the edit: keep the cache aligned so
        // the old state of the paragraph the edit ended in lands at `last`.
        let old = self.states.len();
        if total > old {
            self.states.splice(first..first, std::iter::repeat_n(BlockState::Prose, total - old));
        } else if total < old {
            self.states.drain(first..first + (old - total));
        }
        let ctx = Ctx::new(storage, base, pal);
        storage.beginEditing();
        let (mut i, mut p) = (first, para_at(&buf, start));
        loop {
            let next = ctx.paragraph(&buf, p, self.prev(i), caret, true);
            let changed = std::mem::replace(&mut self.states[i], next) != next;
            if (i >= last && !changed) || p.end == p.content_end {
                break;
            }
            (i, p) = (i + 1, para_at(&buf, p.end));
        }
        storage.endEditing();
        NSRange::new(start, p.end - start)
    }

    /// Restyles the paragraphs the caret left and entered (marker visibility).
    pub fn restyle_caret_move(&mut self, storage: &NSTextStorage, base: &NSFont, pal: &Palette, old: usize, new: usize) {
        let buf = to_u16(&storage.string());
        if self.states.len() != count_paras(&buf) {
            return self.restyle_all(storage, base, pal, new);
        }
        let (a, b) = (locate(&buf, old), locate(&buf, new));
        let ctx = Ctx::new(storage, base, pal);
        storage.beginEditing();
        for (i, start) in if a == b { vec![b] } else { vec![a, b] } {
            ctx.paragraph(&buf, para_at(&buf, start), self.prev(i), new, true);
        }
        storage.endEditing();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn paragraphs_match_nsstring_separators() {
        let b = u("a\r\nb\rc\u{2029}\n");
        assert_eq!(count_paras(&b), 5);
        assert_eq!(locate(&b, 2), (0, 0)); // between CR and LF
        assert_eq!(locate(&b, 3), (1, 3));
        assert_eq!(locate(&b, 8), (4, 8));
        let p = para_at(&b, 0);
        assert_eq!((p.content_end, p.end), (1, 3));
        let p = para_at(&b, 8);
        assert_eq!((p.content_end, p.end), (8, 8));
    }
}
