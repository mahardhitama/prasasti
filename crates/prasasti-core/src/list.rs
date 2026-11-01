//! Tab / Shift+Tab list nesting.

use crate::edit::Edit;
use crate::markdown::{style_paragraph, BlockState};
use crate::Range;

const NL: u16 = b'\n' as u16;
const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const STEP: usize = 4;

/// JS `\s`.
fn is_space(c: u16) -> bool {
    c == 0xFEFF || char::from_u32(c.into()).is_some_and(char::is_whitespace)
}

fn has_content(line: &[u16]) -> bool {
    !line.iter().all(|&c| is_space(c))
}

/// Length of the leading run of spaces and tabs.
fn lead(line: &[u16]) -> usize {
    line.iter().take_while(|&&c| c == SPACE || c == TAB).count()
}

/// Columns the indent spans; a tab runs to the next four-column stop.
fn columns(line: &[u16]) -> usize {
    line[..lead(line)]
        .iter()
        .fold(0, |col, &c| if c == TAB { col + 4 - col % 4 } else { col + 1 })
}

/// `^ {0,3}([-*])([ \t]*\1){2,}[ \t]*$`: "* * *" is a rule, not an item.
fn is_rule(line: &[u16]) -> bool {
    let spaces = line.iter().take_while(|&&c| c == SPACE).count();
    let Some(&m) = line.get(spaces).filter(|_| spaces <= 3) else { return false };
    let rest = &line[spaces + 1..];
    (m == b'-' as u16 || m == b'*' as u16)
        && rest.iter().all(|&c| c == m || c == SPACE || c == TAB)
        && rest.iter().filter(|&&c| c == m).count() >= 2
}

/// `^[ \t]*(?:[-+*]|\d+[.)])[ \t]`, and not a rule.
fn is_list_item(line: &[u16]) -> bool {
    let rest = &line[lead(line)..];
    let digits = rest.iter().take_while(|c| (b'0' as u16..=b'9' as u16).contains(c)).count();
    let marker_end = if digits > 0 {
        match rest.get(digits) {
            Some(&c) if c == b'.' as u16 || c == b')' as u16 => digits + 1,
            _ => return false,
        }
    } else {
        match rest.first() {
            Some(&c) if c < 128 && b"-+*".contains(&(c as u8)) => 1,
            _ => return false,
        }
    };
    matches!(rest.get(marker_end), Some(&c) if c == SPACE || c == TAB) && !is_rule(line)
}

/// Whether `pos` sits in fenced code: the
/// opening fence line counts from its fence on, the closing one up to its end.
fn inside_fence(text: &[u16], pos: usize) -> bool {
    let pos = pos.min(text.len());
    let mut state = BlockState::Prose;
    let mut line_start = 0;
    loop {
        let line_end = text[line_start..]
            .iter()
            .position(|&c| c == NL)
            .map_or(text.len(), |i| line_start + i);
        let line = &text[line_start..line_end];
        let (_, next) = style_paragraph(line, state, None);
        if pos <= line_end || line_end == text.len() {
            let (opens, after) = (next != BlockState::Prose, state != BlockState::Prose);
            let offset = pos - line_start;
            return if opens && !after {
                let fence = line.iter().position(|&c| c == b'`' as u16 || c == b'~' as u16);
                offset >= fence.map_or(0, |i| i + 3).max(2)
            } else if after && !opens {
                offset < line.len()
            } else {
                opens
            };
        }
        state = next;
        line_start = line_end + 1;
    }
}

/// Nests (or, with `outdent`, lifts) the list items `selection` touches, each
/// line moving by the same four-column step so nesting within is kept. Lines
/// that are neither items, their indented continuations, nor blank, and code
/// fences, leave Tab alone: `None` means the caller types a tab. The returned
/// `selection` is the new extent; the caller keeps the caret on the end it was
/// on. A selection that ends at the start of a line leaves that line be.
pub fn indent(text: &[u16], selection: Range, outdent: bool) -> Option<Edit> {
    let len = text.len();
    let sel_start = selection.start.min(len);
    let sel_end = selection.end.clamp(sel_start, len);

    let start = text[..sel_start].iter().rposition(|&c| c == NL).map_or(0, |i| i + 1);
    let from_idx = start.max(sel_end.saturating_sub(1));
    let end = text[from_idx..].iter().position(|&c| c == NL).map_or(len, |i| from_idx + i);
    let lines: Vec<&[u16]> = text[start..end].split(|&c| c == NL).collect();

    // The selection starts at an item and holds only lines that belong to
    // one: other items, blanks, and indented lines.
    let filled: Vec<&[u16]> = lines.iter().copied().filter(|l| has_content(l)).collect();
    if !is_list_item(filled.first()?)
        || filled.iter().any(|l| !is_space(l[0]) && !is_list_item(l))
        || inside_fence(text, start)
        || inside_fence(text, end)
    {
        return None;
    }

    // Shift+Tab lifts no further than the shallowest line can go.
    let step = if outdent {
        -(filled.iter().map(|l| columns(l)).min().unwrap_or(0).min(STEP) as isize)
    } else {
        STEP as isize
    };
    if step == 0 {
        return Some(Edit { range: sel_start..sel_start, replacement: String::new(), selection: sel_start..sel_end });
    }
    let moved: Vec<Vec<u16>> = lines
        .iter()
        .map(|l| {
            if !has_content(l) {
                return l.to_vec();
            }
            let width = (columns(l) as isize + step).max(0) as usize;
            let mut m = vec![SPACE; width];
            m.extend_from_slice(&l[lead(l)..]);
            m
        })
        .collect();
    let nested = moved.join(&NL);

    let new_len = len - (end - start) + nested.len();
    let first_delta = moved[0].len() as isize - lines[0].len() as isize;
    let from = start.max((sel_start as isize + first_delta).max(0) as usize);
    // Neither end of the selection leaves its line, even when the indent it
    // sat in is gone.
    let last_start = start + nested.len() - moved[moved.len() - 1].len();
    let shifted = (sel_end as isize + nested.len() as isize - (end - start) as isize).max(0) as usize;
    let to = from.max(last_start).max(shifted).min(new_len);
    Some(Edit {
        range: start..end,
        replacement: String::from_utf16_lossy(&nested),
        selection: from.min(to)..to,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// A key-press editor: `forward` means the caret is the selection's end.
    struct Ed {
        text: String,
        sel: Range,
        forward: bool,
    }

    impl Ed {
        fn at(text: &str, caret: usize) -> Self {
            Ed { text: text.into(), sel: caret..caret, forward: true }
        }
        /// Anchor at `anchor`, caret dragged to `caret`.
        fn drag(text: &str, anchor: usize, caret: usize) -> Self {
            Ed {
                text: text.into(),
                sel: anchor.min(caret)..anchor.max(caret),
                forward: caret >= anchor,
            }
        }
        fn splice(&mut self, range: Range, replacement: &str, selection: Range) {
            let mut t = u(&self.text);
            t.splice(range, replacement.encode_utf16());
            self.text = String::from_utf16(&t).unwrap();
            self.sel = selection;
        }
        /// Returns whether the key was taken; when not, types a tab.
        fn press(&mut self, outdent: bool) -> bool {
            match indent(&u(&self.text), self.sel.clone(), outdent) {
                Some(e) => {
                    self.splice(e.range, &e.replacement, e.selection);
                    true
                }
                None => {
                    let at = self.sel.start..self.sel.end;
                    let caret = at.start + 1;
                    self.splice(at, "\t", caret..caret);
                    false
                }
            }
        }
        fn tab(&mut self) -> &mut Self {
            self.press(false);
            self
        }
        fn backtab(&mut self) -> &mut Self {
            self.press(true);
            self
        }
        fn caret(&self) -> usize {
            if self.forward { self.sel.end } else { self.sel.start }
        }
        fn selection(&self) -> String {
            format!("{}-{}@{}", self.sel.start, self.sel.end, self.caret())
        }
    }

    #[test]
    fn nests_list_items_on_tab() {
        // An item nests four spaces under the item above, the caret rides
        // along, and Shift+Tab puts it back.
        let mut e = Ed::at("- one\n- two", 11);
        e.tab();
        assert_eq!(e.text, "- one\n    - two");
        assert_eq!(e.caret(), 15);
        e.backtab();
        assert_eq!(e.text, "- one\n- two");
        assert_eq!(e.caret(), 11);

        // The same step nests under ordered markers, the empty one Return
        // leaves included, and Shift+Tab steps back out one level.
        let mut e = Ed::at("1. one\n2. ", 10);
        e.tab();
        assert_eq!(e.text, "1. one\n    2. ");
        assert_eq!(Ed::at("10. a\n- b", 9).tab().text, "10. a\n    - b");
        assert_eq!(
            Ed::at("1. a\n    - b\n        - c", 24).backtab().text,
            "1. a\n    - b\n    - c"
        );

        // The first item nests all the same, an outermost one has nowhere to
        // go, and a tab-indented one comes out like a spaced one.
        let mut e = Ed::at("- a", 3);
        e.tab();
        assert_eq!(e.text, "    - a");
        e.backtab();
        e.backtab();
        assert_eq!(e.text, "- a");
        assert_eq!(Ed::at("- a\n\t- b", 8).backtab().text, "- a\n- b");

        // A selection moves the items it touches together, keeping their
        // depth and its own direction, but not a line it only reaches the
        // start of.
        let mut e = Ed::drag("- a\n- b\n    - c\n- d", 16, 4);
        e.tab();
        assert_eq!(e.text, "- a\n    - b\n        - c\n- d");
        assert_eq!(e.selection(), "8-24@8");

        // Shift+Tab lifts every line by one step, no further than the
        // shallowest line goes, so the nesting within the selection is kept.
        let lift = |text, from, to| Ed::drag(text, from, to).backtab().text.clone();
        assert_eq!(lift("- a\n  - b\n    - c", 6, 17), "- a\n- b\n  - c");
        assert_eq!(lift("- a\n    - b\n  - c", 6, 17), "- a\n  - b\n- c");
        let mut e = Ed::drag("    - a\n    - b", 0, 10);
        e.backtab();
        assert_eq!(e.text, "- a\n- b");
        assert_eq!(e.selection(), "0-4@4");
        assert_eq!(lift("- a\n \t- b\n    - c", 4, 15), "- a\n- b\n- c");

        // Indented continuation lines move with their item, but a paragraph
        // at the margin is no item, so Tab leaves the list alone.
        assert_eq!(
            Ed::drag("- a\n- b\n  more", 4, 14).tab().text,
            "- a\n    - b\n      more"
        );
        assert!(!Ed::drag("- a\n- b\n\nParagraph", 4, 18).press(false));

        // A fence indented under an item moves with it, but one that starts
        // the selection, or a selection running from a list into a code
        // fence, is left alone.
        assert_eq!(
            Ed::drag("- a\n- b\n  ```\n  code\n  ```", 4, 26).tab().text,
            "- a\n    - b\n      ```\n      code\n      ```"
        );
        assert!(!Ed::drag("  ```\n  - x\n  ```", 0, 17).press(false));
        assert!(!Ed::drag("- a\n  ```\n  - x\n  ```", 0, 14).press(false));

        // Outside a list, on a rule, and inside a code fence, Tab types a tab.
        assert_eq!(Ed::at("plain", 5).tab().text, "plain\t");
        assert_eq!(Ed::at("\n- a", 0).tab().text, "\t\n- a");
        assert_eq!(Ed::at("- a\n* * *", 9).tab().text, "- a\n* * *\t");
        assert_eq!(Ed::at("```\n- a\n```", 7).tab().text, "```\n- a\t\n```");
    }

    #[test]
    fn out_of_range_selection_clamps() {
        let e = indent(&u("- a"), Range { start: 9, end: 2 }, false).unwrap();
        assert_eq!(e.range, 0..3);
        assert_eq!(e.replacement, "    - a");
        assert!(indent(&[], 5..9, false).is_none());
    }
}
