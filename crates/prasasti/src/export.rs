//! Markdown to HTML, for the pasteboard and for printing.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
use objc2_foundation::NSString;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

const OPTIONS: Options = Options::ENABLE_TABLES.union(Options::ENABLE_STRIKETHROUGH).union(Options::ENABLE_TASKLISTS);

/// The Markdown as an HTML fragment. Raw HTML in the source passes through
/// unchanged (a personal tool: the author is the only source).
pub fn to_html(markdown: &str) -> String {
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(markdown, OPTIONS));
    out
}

/// Automatic heading numbers for Preview and print: h1 "1.", h2 "1.1.", down
/// to h5; h6 gets none. A skipped level counts as 0 ("1.0.1.").
#[derive(Default)]
pub struct HeadingNumbers([u32; 5]);

impl HeadingNumbers {
    /// The number of the next heading at `level`.
    pub fn next(&mut self, level: HeadingLevel) -> Option<String> {
        let i = level as usize - 1;
        if i >= self.0.len() {
            return None;
        }
        self.0[i] += 1;
        self.0[i + 1..].fill(0);
        Some(self.0[..=i].iter().map(|n| format!("{n}.")).collect())
    }
}

/// First private-use code point of the image markers in [`print_html`]: marker `i` is `IMAGE_MARK + i`.
pub const IMAGE_MARK: u32 = 0xF0000;

/// A local image the print page leaves to the caller: its file and alt text.
#[derive(Debug, PartialEq)]
pub struct PrintImage {
    pub path: PathBuf,
    pub alt: String,
}

/// A complete page for `initWithHTML:`: UTF-8 declared, the editor `family`
/// and black text (the print view is white paper whatever the appearance).
/// Each local image becomes the marker `IMAGE_MARK + i` for `images[i]`: the
/// importer can't load files in the bundled app, so the caller draws them.
/// Relative paths resolve in `doc_dir`; with no `doc_dir` they stay `<img>`.
pub fn print_html(markdown: &str, family: &str, doc_dir: Option<&Path>) -> (String, Vec<PrintImage>) {
    // The family sits in a CSS string inside an HTML attribute: escape for both.
    let css = family.replace('\\', "\\\\").replace('\'', "\\'");
    let attr = css.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    let mut images: Vec<PrintImage> = Vec::new();
    let mut events = Vec::new();
    let mut alt: Option<String> = None;
    let mut numbers = HeadingNumbers::default();
    for e in Parser::new_ext(markdown, OPTIONS) {
        match e {
            Event::End(TagEnd::Image) if alt.is_some() => {
                if let Some(img) = images.last_mut() {
                    img.alt = alt.take().unwrap_or_default();
                }
                let mark = char::from_u32(IMAGE_MARK + images.len() as u32 - 1).unwrap_or('\u{FFFD}');
                events.push(Event::InlineHtml(mark.to_string().into()));
            }
            Event::Text(t) | Event::Code(t) if alt.is_some() => alt.as_mut().unwrap().push_str(&t),
            _ if alt.is_some() => {}
            Event::Start(Tag::Image { ref dest_url, .. }) => match local_path(dest_url, doc_dir) {
                Some(path) => {
                    images.push(PrintImage { path, alt: String::new() });
                    alt = Some(String::new());
                }
                None => events.push(e),
            },
            // The importer keeps no margin-top, left margin or block background
            // outside a table cell, so headings, code and quotes sit in one-cell tables.
            Event::Start(Tag::Heading { level, .. }) => {
                events.push(panel(&heading_panel(level)));
                events.push(e);
                if let Some(n) = numbers.next(level) {
                    events.push(Event::InlineHtml(format!("<span style=\"color:#888\">{n} </span>").into()));
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                events.push(panel("margin-top:8pt\"><tr><td style=\"border:none;background:#f3f3f3;padding:6pt 9pt"));
                events.push(e);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                events.push(panel("\"><tr><td style=\"border:none;border-left:2px solid #bbb;padding:0 0 0 10pt;color:#444"));
                events.push(e);
            }
            Event::End(TagEnd::Heading(_)) => {
                events.push(e);
                // A table's margin-bottom is dropped too: a 5 pt line keeps text off the rule.
                events.push(Event::Html("</td></tr></table>\n<p style=\"margin:0;font-size:5pt\">&#8203;</p>\n".into()));
            }
            Event::End(TagEnd::CodeBlock | TagEnd::BlockQuote(_)) => {
                events.push(e);
                events.push(Event::Html("</td></tr></table>\n".into()));
            }
            e => events.push(e),
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    let page = format!("<html><head><meta charset=\"utf-8\"><style>{PRINT_CSS}</style></head><body style=\"font-family:'{attr}';color:#000\">{body}</body></html>");
    (page, images)
}

/// Grid lines round every cell, as in Preview (the importer turns these into NSTextTable borders);
/// code in the bundled mono face; h6, which has no number, in italics.
const PRINT_CSS: &str = "table{border-collapse:collapse}th,td{border:1px solid #000;padding:2px 6px}\
th{background:#f3f3f3}pre{margin:0}code{font-family:'JetBrains Mono',Menlo,monospace;font-size:0.9em}\
h1,h2,h3,h4,h5,h6{margin:0}h6{font-style:italic}";

/// Opens a full-width one-cell table: `style` ends the table's style and may hold the cell's.
fn panel(style: &str) -> Event<'static> {
    Event::Html(format!("<table style=\"width:100%;{style}\">").into())
}

/// Space above each heading, and its rule: h1–h3 only, fainter at each level.
fn heading_panel(level: HeadingLevel) -> String {
    let (above, rule) = match level {
        HeadingLevel::H1 => (22, "border-bottom:1px solid #666;"),
        HeadingLevel::H2 => (18, "border-bottom:1px solid #999;"),
        HeadingLevel::H3 => (14, "border-bottom:1px solid #ccc;"),
        _ => (10, ""),
    };
    format!("margin-top:{above}pt\"><tr><td style=\"border:none;{rule}padding:0 0 2pt")
}

/// The file an image destination names: `file://`, absolute, or relative to `doc_dir`.
/// shortcut: no percent-decoding (`a%20b.png`), Preview's URL base does it; add if users hit it.
fn local_path(dest: &str, doc_dir: Option<&Path>) -> Option<PathBuf> {
    let dest = dest.split(['?', '#']).next()?;
    if let Some(abs) = dest.strip_prefix("file://") {
        return Some(PathBuf::from(abs));
    }
    if dest.is_empty() || dest.contains("://") || dest.starts_with("data:") || dest.starts_with("//") {
        return None;
    }
    let p = Path::new(dest);
    if p.is_absolute() { Some(p.to_path_buf()) } else { doc_dir.map(|d| d.join(p)) }
}

/// Replaces the pasteboard's contents with the rendered HTML and the Markdown as the plain-text fallback.
pub fn copy_as_html(pb: &NSPasteboard, markdown: &str) {
    pb.clearContents();
    // SAFETY: AppKit's constant pasteboard types.
    let (html_type, string_type) = unsafe { (NSPasteboardTypeHTML, NSPasteboardTypeString) };
    pb.setString_forType(&NSString::from_str(&to_html(markdown)), html_type);
    pb.setString_forType(&NSString::from_str(markdown), string_type);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_strike() {
        assert!(to_html("~~x~~").contains("<del>x</del>"));
    }

    #[test]
    fn renders_tables_and_tasks() {
        assert!(to_html("|a|\n|-|\n|b|\n").contains("<table>"));
        assert!(to_html("- [x] t\n").contains("checkbox"));
    }

    #[test]
    fn heading_numbers_nest_to_h5() {
        use HeadingLevel::*;
        let mut n = HeadingNumbers::default();
        let got: Vec<_> = [H1, H2, H2, H3, H5, H6, H1, H3].map(|l| n.next(l)).into_iter().collect();
        let want = ["1.", "1.1.", "1.2.", "1.2.1.", "1.2.1.0.1.", "", "2.", "2.0.1."];
        assert_eq!(got, want.map(|w| (!w.is_empty()).then(|| w.to_string())));
    }

    #[test]
    fn print_numbers_headings_and_rules_h1_to_h3() {
        let (h, _) = print_html("# A\n## B\n#### C\n###### D\n", "F", None);
        let num = |n| format!(r#"<span style="color:#888">{n} </span>"#);
        assert!(h.contains(&format!("<h1>{}A</h1>\n</td></tr></table>\n<p style", num("1."))), "{h}");
        assert!(h.contains(&format!("solid #999;padding:0 0 2pt\">\n<h2>{}B</h2>", num("1.1."))), "{h}");
        assert!(h.contains(&format!("<td style=\"border:none;padding:0 0 2pt\">\n<h4>{}C</h4>", num("1.1.0.1."))), "{h}");
        assert!(h.contains("<h6>D</h6>"), "{h}");
    }

    #[test]
    fn print_puts_code_and_quotes_in_panels() {
        let (h, _) = print_html("> q\n\n```\nc\n```\n", "F", None);
        assert!(h.contains("border-left:2px solid #bbb;padding:0 0 0 10pt;color:#444\">\n<blockquote>"), "{h}");
        assert!(h.contains("background:#f3f3f3;padding:6pt 9pt\">\n<pre><code>c\n</code></pre>\n</td></tr></table>"), "{h}");
    }

    #[test]
    fn family_cannot_break_out_of_the_style_attribute() {
        let (h, _) = print_html("x", "A'\"B\\<", None);
        assert!(h.contains(r"font-family:'A\'&quot;B\\&lt;'"), "{h}");
        assert_eq!(h.matches('"').count(), 4); // charset pair + style pair, no stray quote
    }

    #[test]
    fn print_marks_local_images_only() {
        let md = "![a *b*](x.png) ![](/abs/y.png) ![](file:///f/z.png) ![r](https://h/r.png) ![d](data:image/png;base64,AA)\n";
        let (h, imgs) = print_html(md, "F", Some(Path::new("/doc")));
        let paths: Vec<_> = imgs.iter().map(|i| i.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["/doc/x.png", "/abs/y.png", "/f/z.png"]);
        assert_eq!(imgs[0].alt, "a b");
        for i in 0..3 {
            assert!(h.contains(char::from_u32(IMAGE_MARK + i).unwrap()), "{h}");
        }
        assert!(h.contains("https://h/r.png") && h.contains("data:image"), "{h}");
        assert!(!h.contains("x.png"), "{h}");
        // Untitled: a relative path has nothing to resolve against.
        let (h, imgs) = print_html("![](x.png)\n", "F", None);
        assert!(imgs.is_empty() && h.contains("x.png"), "{h}");
    }
}
