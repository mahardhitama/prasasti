//! Markdown to HTML, for the pasteboard and for printing.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
use objc2_foundation::NSString;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};

const OPTIONS: Options = Options::ENABLE_TABLES.union(Options::ENABLE_STRIKETHROUGH).union(Options::ENABLE_TASKLISTS);

/// The Markdown as an HTML fragment. Raw HTML in the source passes through
/// unchanged (a personal tool: the author is the only source).
pub fn to_html(markdown: &str) -> String {
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(markdown, OPTIONS));
    out
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
            e => events.push(e),
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    let page = format!("<html><head><meta charset=\"utf-8\"><style>{TABLE_CSS}</style></head><body style=\"font-family:'{attr}';color:#000\">{body}</body></html>");
    (page, images)
}

/// Grid lines round every cell, as in Preview (the importer turns these into NSTextTable borders).
const TABLE_CSS: &str = "table{border-collapse:collapse}th,td{border:1px solid #000;padding:2px 6px}";

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
