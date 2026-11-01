//! Markdown to HTML, for the pasteboard and for printing.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
use objc2_foundation::NSString;
use pulldown_cmark::{Options, Parser, html};

/// The Markdown as an HTML fragment. Raw HTML in the source passes through
/// unchanged (a personal tool: the author is the only source).
pub fn to_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(markdown, options));
    out
}

/// A complete page for `initWithHTML:`: UTF-8 declared, the editor `family`
/// and black text (the print view is white paper whatever the appearance).
pub fn print_html(markdown: &str, family: &str) -> String {
    // The family sits in a CSS string inside an HTML attribute: escape for both.
    let css = family.replace('\\', "\\\\").replace('\'', "\\'");
    let attr = css.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<html><head><meta charset=\"utf-8\"></head><body style=\"font-family:'{attr}';color:#000\">{}</body></html>",
        to_html(markdown)
    )
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
        let h = print_html("x", "A'\"B\\<");
        assert!(h.contains(r"font-family:'A\'&quot;B\\&lt;'"), "{h}");
        assert_eq!(h.matches('"').count(), 4); // charset pair + style pair, no stray quote
    }
}
