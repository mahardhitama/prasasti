//! Preview: the Markdown as a full page, the `prasasti:` scheme that serves
//! its assets and local images, and the read-only web view that shows it.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSResponder, NSView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSBundle, NSData, NSDate, NSDictionary, NSError, NSHTTPURLResponse, NSObject,
    NSObjectProtocol, NSRect, NSRunLoop, NSString, NSURL, NSURLErrorBadURL, NSURLErrorDomain, ns_string,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType,
    WKScriptMessage, WKScriptMessageHandler, WKURLSchemeHandler, WKURLSchemeTask, WKUserContentController, WKWebView,
    WKWebViewConfiguration,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};

use crate::editor::EditorView;
use crate::util::guarded;

pub const CSP: &str = "default-src 'none'; script-src prasasti://app; style-src prasasti://app 'unsafe-inline'; img-src * data: file: prasasti:; font-src prasasti://app";

/// Local files the page may load: images only, so a document cannot pull other files in.
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("heic", "image/heic"),
    ("tiff", "image/tiff"),
    ("bmp", "image/bmp"),
    ("avif", "image/avif"),
];
const APP_TYPES: &[(&str, &str)] =
    &[("js", "text/javascript"), ("css", "text/css"), ("woff2", "font/woff2"), ("ttf", "font/ttf")];

/// The bundled editor font, served to the page under its own name: WebKit's
/// web content process cannot see fonts registered only for the app.
const BUNDLED_FONT: &str = "Prasasti JetBrains Mono";
const BUNDLED_FACES: [(&str, &str, &str); 4] = [
    ("Regular", "normal", "normal"),
    ("Bold", "bold", "normal"),
    ("Italic", "normal", "italic"),
    ("BoldItalic", "bold", "italic"),
];

/// Raw HTML with every `src=file://…` (any quoting) pointing at `prasasti://file/…`.
fn rewrite_raw_src(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for (i, _) in lower.match_indices("src=") {
        let mut at = i + "src=".len();
        if lower[at..].starts_with(['"', '\'']) {
            at += 1;
        }
        if lower[at..].starts_with("file://") {
            out.push_str(&html[last..at]);
            out.push_str("prasasti://file");
            last = at + "file://".len();
        }
    }
    out.push_str(&html[last..]);
    out
}

/// The whole page: CSP, both palettes, the column, the app's scripts and the
/// Markdown body. `file://` image URLs become `prasasti://file/…`.
pub fn page_html(markdown: &str, font_family: &str, font_px: f64, columns: usize) -> String {
    let options = Options::ENABLE_MATH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    let mut events = Vec::new();
    let mut in_lone_math = false;
    for (e, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match e {
            // Its content was replaced by the DisplayMath below.
            _ if in_lone_math => {
                if e == Event::End(TagEnd::Paragraph) {
                    in_lone_math = false;
                    events.push(e);
                }
            }
            Event::Start(Tag::Paragraph) => {
                events.push(e);
                if let Some(tex) = lone_display_math(&markdown[range]) {
                    events.push(Event::DisplayMath(tex.to_string().into()));
                    in_lone_math = true;
                }
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) if dest_url.starts_with("file://") => {
                let dest_url = format!("prasasti://file{}", &dest_url["file://".len()..]).into();
                events.push(Event::Start(Tag::Image { link_type, dest_url, title, id }));
            }
            Event::Html(h) => events.push(Event::Html(rewrite_raw_src(&h).into())),
            Event::InlineHtml(h) => events.push(Event::InlineHtml(rewrite_raw_src(&h).into())),
            e => events.push(e),
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    // A CSS string inside <style>: no quote, backslash or `<` (`</style>`) gets out.
    let family = font_family.replace('\\', "\\\\").replace('\'', "\\'").replace('<', "\\3c ");
    // Code blocks always use the bundled face; only the body switches on the setting.
    let faces: String = BUNDLED_FACES.iter().map(|(file, weight, style)| {
        format!("@font-face {{ font-family: '{BUNDLED_FONT}'; src: url(prasasti://app/fonts/JetBrainsMono-{file}.ttf); font-weight: {weight}; font-style: {style}; }}\n")
    }).collect();
    let first = if font_family == crate::settings::DEFAULT_FAMILY { format!("'{BUNDLED_FONT}', ") } else { String::new() };
    let vars = |dark| {
        let names = ["bg", "text", "accent", "marker", "code-bg", "selection", "search", "current"];
        let hex = crate::theme::hex(dark);
        names.iter().zip(hex).map(|(n, h)| format!("--{n}: #{h:06x};")).collect::<String>()
    };
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{CSP}">
<meta name="color-scheme" content="light dark">
<link rel="stylesheet" href="prasasti://app/katex/katex.min.css">
<link rel="stylesheet" href="prasasti://app/preview.css">
<style>
{faces}:root {{ {light} }}
@media (prefers-color-scheme: dark) {{ :root {{ {dark} }} }}
body {{ max-width: {columns}ch; margin: 0 auto; font-family: {first}'{family}', ui-monospace, monospace; font-size: {font_px}px; }}
</style>
<script src="prasasti://app/katex/katex.min.js"></script>
<script src="prasasti://app/highlight/highlight.min.js"></script>
<script type="module" src="prasasti://app/preview.js"></script>
</head><body>
{body}</body></html>
"#,
        light = vars(false),
        dark = vars(true),
    )
}

/// The TeX of a one-line paragraph that is only `$$…$$`. pulldown-cmark takes
/// math only with balanced braces; this keeps an unbalanced one math, so KaTeX
/// shows the error in its place instead of the raw dollars.
fn lone_display_math(src: &str) -> Option<&str> {
    let tex = src.trim().strip_prefix("$$")?.strip_suffix("$$")?;
    (!tex.is_empty() && !tex.contains(['$', '\n'])).then_some(tex)
}

/// The page's base URL: relative and absolute image paths resolve under
/// `prasasti://file/` with no rewriting.
pub fn base_url(doc_dir: Option<&Path>) -> String {
    let mut s = String::from("prasasti://file");
    for &b in doc_dir.map(|d| d.as_os_str().as_bytes()).unwrap_or_default() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

fn decode(s: &str) -> Option<OsString> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(OsString::from_vec(out))
}

fn mime(types: &[(&str, &'static str)], p: &Path) -> Option<&'static str> {
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    types.iter().find(|(e, _)| *e == ext).map(|t| t.1)
}

/// `prasasti://app/<rel>` inside `bundle_preview_dir` (never above it), or
/// `prasasti://file/<abs>` when it is an image; the path and its MIME type.
pub fn resolve(url: &str, bundle_preview_dir: &Path) -> Option<(PathBuf, &'static str)> {
    let url = url.split(['?', '#']).next()?;
    let normal = |p: &Path| p.components().all(|c| matches!(c, Component::Normal(_) | Component::RootDir));
    if let Some(rel) = url.strip_prefix("prasasti://app/") {
        let rel = PathBuf::from(decode(rel)?);
        if rel.is_absolute() || !normal(&rel) {
            return None;
        }
        let ty = mime(APP_TYPES, &rel)?;
        Some((bundle_preview_dir.join(rel), ty))
    } else if let Some(abs) = url.strip_prefix("prasasti://file/") {
        let mut p = OsString::from("/");
        p.push(decode(abs)?);
        let p = PathBuf::from(p);
        if !normal(&p) {
            return None;
        }
        let ty = mime(IMAGE_TYPES, &p)?;
        Some((p, ty))
    } else {
        None
    }
}

/// The bundle's `preview/` folder, or the source tree's when there is no bundle (tests).
fn asset_root() -> PathBuf {
    NSBundle::mainBundle()
        .resourcePath()
        .map(|p| PathBuf::from(p.to_string()).join("preview"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/preview"))
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PathBuf]
    struct SchemeHandler;

    unsafe impl NSObjectProtocol for SchemeHandler {}

    unsafe impl WKURLSchemeHandler for SchemeHandler {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            guarded(|| self.serve(task));
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {
            // Nothing to cancel: start answers every task before returning.
        }
    }
);

impl SchemeHandler {
    fn new(mtm: MainThreadMarker, root: PathBuf) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(root);
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// Answers with the file (200) or an empty 404. App assets allow any
    /// origin: the page's origin is `prasasti://file`, and module scripts
    /// and fonts from `prasasti://app` are cross-origin loads.
    fn serve(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        // SAFETY: a plain getter.
        let Some(url) = unsafe { task.request() }.URL() else { return fail(task) };
        let s = url.absoluteString().map(|s| s.to_string()).unwrap_or_default();
        // shortcut: a synchronous read on the main thread (small local files);
        // move it to a background queue if large images ever stall the UI.
        let found = resolve(&s, self.ivars()).and_then(|(p, ty)| Some((std::fs::read(p).ok()?, ty)));
        let (status, ty, bytes) = match found {
            Some((bytes, ty)) => (200, ty, bytes),
            None => (404, "text/plain", Vec::new()),
        };
        let headers = NSDictionary::from_slices(
            &[ns_string!("Content-Type"), ns_string!("Access-Control-Allow-Origin")],
            &[&*NSString::from_str(ty), ns_string!("*")],
        );
        let Some(response) = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
            NSHTTPURLResponse::alloc(),
            &url,
            status,
            Some(ns_string!("HTTP/1.1")),
            Some(&headers),
        ) else {
            return fail(task);
        };
        // SAFETY: response, data, finish: once each, in order, on a live task.
        unsafe {
            task.didReceiveResponse(&response);
            task.didReceiveData(&NSData::with_bytes(&bytes));
            task.didFinish();
        }
    }
}

/// Ends a task that cannot be answered, so the page never waits on it.
fn fail(task: &ProtocolObject<dyn WKURLSchemeTask>) {
    // SAFETY: a constant domain; the task is live and not yet answered.
    unsafe {
        let error = NSError::errorWithDomain_code_userInfo(NSURLErrorDomain, NSURLErrorBadURL, None);
        task.didFailWithError(&error);
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop. Holds the
    // editor weakly: the content controller retains this handler, never the view.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<EditorView>]
    struct CopyHandler;

    unsafe impl NSObjectProtocol for CopyHandler {}

    unsafe impl WKScriptMessageHandler for CopyHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _ucc: &WKUserContentController, message: &WKScriptMessage) {
            guarded(|| {
                // SAFETY: a plain getter; the body is page input, so it is checked below.
                let body = unsafe { message.body() };
                if let Some(text) = copy_text(&body) {
                    let pb = NSPasteboard::generalPasteboard();
                    pb.clearContents();
                    // SAFETY: a constant type name.
                    pb.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
                    if let Some(pill) = self.ivars().load().and_then(|e| e.status()) {
                        pill.flash("Copied to clipboard");
                    }
                }
            });
        }
    }
);

impl CopyHandler {
    fn new(mtm: MainThreadMarker, editor: &EditorView) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Weak::new(editor));
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// The text of a `{type: "copy", text: String}` page message; anything else is ignored.
fn copy_text(body: &AnyObject) -> Option<String> {
    let dict = body.downcast_ref::<NSDictionary>()?;
    let get = |k: &NSString| dict.objectForKey(k);
    let kind = get(ns_string!("type"))?;
    let text = get(ns_string!("text"))?;
    (kind.downcast_ref::<NSString>()?.to_string() == "copy").then(|| text.downcast_ref::<NSString>().map(|t| t.to_string()))?
}

pub struct Ivars {
    editor: Weak<EditorView>,
    _handler: Retained<SchemeHandler>,
    /// The last page and base URL loaded.
    html: RefCell<String>,
    base: RefCell<String>,
    /// 1 while the newest `load`'s navigation is still to be allowed.
    pending: Cell<u32>,
    /// The error page is showing (so its own failure does not loop).
    failed: Cell<bool>,
}

define_class!(
    // SAFETY: WKWebView has no subclassing requirements we violate; no Drop.
    #[unsafe(super(WKWebView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct PreviewView;

    unsafe impl NSObjectProtocol for PreviewView {}

    unsafe impl WKNavigationDelegate for PreviewView {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let policy = guarded(|| self.policy(action)).unwrap_or(WKNavigationActionPolicy::Cancel);
            handler.call((policy,));
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _web: &WKWebView, _nav: Option<&WKNavigation>, error: &NSError) {
            guarded(|| self.load_failed(error));
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web: &WKWebView, _nav: Option<&WKNavigation>, error: &NSError) {
            guarded(|| self.load_failed(error));
        }
    }

    impl PreviewView {
        /// ⌘-scroll zooms (the editor's own accumulation and steps); plain scroll scrolls.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            guarded(|| -> () {
                let editor = self.ivars().editor.load();
                match editor.filter(|_| event.modifierFlags().contains(NSEventModifierFlags::Command)) {
                    // SAFETY: NSResponder's scrollWheel:, same signature.
                    Some(e) => unsafe { msg_send![&*e, scrollWheel: event] },
                    // SAFETY: WKWebView's own implementation, same signature.
                    None => unsafe { msg_send![super(self), scrollWheel: event] },
                }
            });
        }

        /// The page is read-only: nothing that would edit is enabled.
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            guarded(|| {
                let editing = [sel!(paste:), sel!(pasteAsPlainText:), sel!(cut:), sel!(delete:), sel!(undo:), sel!(redo:)];
                if item.action().is_some_and(|a| editing.contains(&a)) {
                    return false;
                }
                // SAFETY: WKWebView's NSUserInterfaceValidations method; a menu item is a validated item.
                unsafe { msg_send![super(self), validateUserInterfaceItem: item] }
            })
            .unwrap_or(false)
        }
    }
);

impl PreviewView {
    /// A web view serving `prasasti:`; ⌘-scroll goes to `editor`.
    pub fn new(mtm: MainThreadMarker, frame: NSRect, editor: &EditorView) -> Retained<Self> {
        // SAFETY: a fresh configuration.
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let handler = SchemeHandler::new(mtm, asset_root());
        // SAFETY: "prasasti" is not a scheme WebKit handles itself.
        unsafe { config.setURLSchemeHandler_forURLScheme(Some(ProtocolObject::from_ref(&*handler)), ns_string!("prasasti")) };
        // The configuration's controller retains the handler for the web view's life.
        // SAFETY: a fresh controller; the page posts to `webkit.messageHandlers.prasasti`.
        unsafe {
            config.userContentController().addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*CopyHandler::new(mtm, editor)),
                ns_string!("prasasti"),
            )
        };
        let this = Self::alloc(mtm).set_ivars(Ivars {
            editor: Weak::new(editor),
            _handler: handler,
            html: RefCell::default(),
            base: RefCell::default(),
            pending: Cell::new(0),
            failed: Cell::new(false),
        });
        // SAFETY: WKWebView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame, configuration: &*config] };
        // SAFETY: the delegate is the view itself (held weakly, no cycle).
        unsafe { this.setNavigationDelegate(Some(ProtocolObject::from_ref(&*this))) };
        this
    }

    /// Shows `html` with `base` as its URL, unless that page already shows.
    pub fn render(&self, html: &str, base: &str) {
        let iv = self.ivars();
        if !iv.failed.get() && *iv.html.borrow() == html && *iv.base.borrow() == base {
            return;
        }
        iv.failed.set(false);
        *iv.html.borrow_mut() = html.into();
        self.load(html, base);
    }

    /// The last page handed to `render`.
    pub fn html(&self) -> String {
        self.ivars().html.borrow().clone()
    }

    fn load(&self, html: &str, base: &str) {
        let iv = self.ivars();
        *iv.base.borrow_mut() = base.into();
        // Only the newest load may navigate; a superseded one leaves no allowance.
        iv.pending.set(1);
        let url = NSURL::URLWithString(&NSString::from_str(base));
        // SAFETY: a string and an optional base URL.
        let _ = unsafe { self.loadHTMLString_baseURL(&NSString::from_str(html), url.as_deref()) };
    }

    /// Only our own loads and same-page `#fragment` jumps; a clicked link
    /// opens outside instead.
    fn policy(&self, action: &WKNavigationAction) -> WKNavigationActionPolicy {
        let iv = self.ivars();
        // SAFETY: plain getters.
        let (url, kind, main) = unsafe {
            (action.request().URL(), action.navigationType(), action.targetFrame().is_some_and(|f| f.isMainFrame()))
        };
        let s = url.as_ref().and_then(|u| u.absoluteString()).map(|s| s.to_string()).unwrap_or_default();
        if kind == WKNavigationType::Other && main && iv.pending.get() > 0 && s == *iv.base.borrow() {
            iv.pending.set(iv.pending.get() - 1);
            return WKNavigationActionPolicy::Allow;
        }
        let page = |u: &str| u.split('#').next().unwrap_or_default().to_string();
        // SAFETY: a plain getter.
        let current = unsafe { self.URL() }.and_then(|u| u.absoluteString()).map(|c| page(&c.to_string()));
        if s.contains('#') && current == Some(page(&s)) {
            return WKNavigationActionPolicy::Allow;
        }
        if kind == WKNavigationType::LinkActivated {
            open_outside(&s);
        }
        WKNavigationActionPolicy::Cancel
    }

    /// A load that failed for any reason but our own cancel shows a plain message.
    fn load_failed(&self, error: &NSError) {
        let iv = self.ivars();
        let domain = error.domain().to_string();
        // NSURLErrorCancelled (superseded load) and WebKit's "interrupted by policy change" (our Cancel).
        if iv.failed.get() || error.code() == -999 || (domain == "WebKitErrorDomain" && error.code() == 102) {
            return;
        }
        iv.failed.set(true);
        let msg = error.localizedDescription().to_string().replace('&', "&amp;").replace('<', "&lt;");
        let base = iv.base.borrow().clone();
        self.load(&format!("<!DOCTYPE html><meta charset=\"utf-8\"><p>Couldn't show the preview: {msg}</p>"), &base);
    }

    /// Test hook: runs `js` in the page and pumps the run loop (up to
    /// `timeout` seconds) for its result, as a string.
    pub fn eval(&self, js: &str, timeout: f64) -> Option<String> {
        let out: Rc<RefCell<Option<Option<String>>>> = Rc::default();
        let o = out.clone();
        let done = RcBlock::new(move |r: *mut AnyObject, e: *mut NSError| {
            // SAFETY: a null or valid result object from WebKit.
            let r = unsafe { r.as_ref() }.filter(|_| e.is_null()).map(|r| {
                // SAFETY: every object answers description.
                let d: Retained<NSString> = unsafe { msg_send![r, description] };
                d.to_string()
            });
            *o.borrow_mut() = Some(r);
        });
        // SAFETY: a script string and a completion block.
        unsafe { self.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&done)) };
        let start = std::time::Instant::now();
        while out.borrow().is_none() && start.elapsed().as_secs_f64() < timeout {
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.02));
        }
        out.take().flatten()
    }
}

/// Types that run something when opened, whatever their permissions.
const LAUNCHERS: &[&str] = &[
    "app", "command", "sh", "bash", "zsh", "csh", "ksh", "tool", "terminal", "workflow", "action", "pkg", "mpkg",
    "dmg", "scpt", "scptd", "applescript", "jar", "webloc", "inetloc", "fileloc", "url", "prefpane", "saver",
    "qlgenerator", "kext", "mobileconfig", "py", "rb", "pl", "osax", "plugin", "bundle", "framework",
];

/// Whether a linked file may be opened in its app: a regular file, not
/// executable, not a launcher type. Anything else is only shown in Finder.
fn opens_directly(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(p) else { return false };
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    meta.is_file() && meta.permissions().mode() & 0o111 == 0 && !LAUNCHERS.contains(&ext.as_str())
}

/// Opens a clicked link outside: web and mail links in their apps; a local
/// file (`prasasti://file/…`) in its app only when it cannot launch
/// anything, otherwise revealed in Finder.
fn open_outside(url: &str) {
    let ws = NSWorkspace::sharedWorkspace();
    if ["http://", "https://", "mailto:"].iter().any(|p| url.starts_with(p)) {
        if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
            ws.openURL(&u);
        }
        return;
    }
    let Some(rest) = url.strip_prefix("prasasti://file/") else { return };
    let Some(path) = decode(rest.split(['?', '#']).next().unwrap_or_default()) else { return };
    let mut abs = OsString::from("/");
    abs.push(path);
    let path = PathBuf::from(abs);
    let Some(u) = path.to_str().map(|p| NSURL::fileURLWithPath(&NSString::from_str(p))) else { return };
    if opens_directly(&path) {
        ws.openURL(&u);
    } else {
        ws.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[u]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(md: &str) -> String {
        page_html(md, "JetBrains Mono", 15.0, 80)
    }

    #[test]
    fn math_spans_emitted() {
        assert!(page("$a$").contains(r#"<span class="math math-inline">a</span>"#));
        assert!(page("$$b$$").contains(r#"<span class="math math-display">b</span>"#));
        // KaTeX gets the raw TeX: backslash escapes inside math are not eaten.
        assert!(page(r"$$x^2\,dx \\ y$$").contains(r"x^2\,dx \\ y"), "{}", page(r"$$x^2\,dx \\ y$$"));
    }

    #[test]
    fn unbalanced_display_math_still_math() {
        // pulldown-cmark only takes balanced braces as math; KaTeX must still get this to report it.
        let p = page("Before:\n\n$$\\frac{1}{$$\n\nAfter $5 and $$6.");
        assert!(p.contains(r#"<p><span class="math math-display">\frac{1}{</span></p>"#), "{p}");
        assert!(p.contains("After $5 and $$6."), "{p}");
    }

    #[test]
    fn mermaid_fence_kept() {
        assert!(page("```mermaid\nflowchart TD\n```\n").contains(r#"<code class="language-mermaid">"#));
    }

    #[test]
    fn page_has_csp_and_column() {
        let p = page("x");
        assert!(p.contains(&format!(r#"content="{CSP}""#)), "{p}");
        assert!(p.contains("max-width: 80ch"), "{p}");
    }

    #[test]
    fn family_cannot_close_the_style_element() {
        let p = page_html("x", "A'</style><script>", 15.0, 80);
        assert!(!p.contains("</style><script>"), "{p}");
    }

    #[test]
    fn file_url_image_rewritten() {
        assert!(page("![a](file:///x/y.png)").contains(r#"src="prasasti://file/x/y.png""#));
    }

    #[test]
    fn raw_html_file_image_rewritten() {
        let p = page("<img src=\"file:///x/y.png\">\n\nInline <img SRC='file:///a/b.png'> here.");
        assert!(p.contains(r#"<img src="prasasti://file/x/y.png">"#), "{p}");
        assert!(p.contains("<img SRC='prasasti://file/a/b.png'>"), "{p}");
    }

    #[test]
    fn bundled_font_served_to_the_page() {
        let p = page("x");
        assert!(p.contains("url(prasasti://app/fonts/JetBrainsMono-BoldItalic.ttf)"), "{p}");
        assert!(p.contains("font-family: 'Prasasti JetBrains Mono', 'JetBrains Mono',"), "{p}");
        let menlo = page_html("x", "Menlo", 15.0, 80);
        assert!(menlo.contains("@font-face") && menlo.contains("font-family: 'Menlo', ui-monospace"), "faces stay for code blocks");
        assert_eq!(resolve("prasasti://app/fonts/JetBrainsMono-Regular.ttf", Path::new("/r")).map(|r| r.1), Some("font/ttf"));
    }

    #[test]
    fn only_plain_files_open_directly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("prasasti-open-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("A.app")).unwrap();
        let file = |name: &str, mode: u32| {
            let p = dir.join(name);
            std::fs::write(&p, "x").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        };
        assert!(opens_directly(&file("a.png", 0o644)));
        assert!(opens_directly(&file("notes.md", 0o644)));
        assert!(!opens_directly(&file("run.png", 0o755)), "executable bit");
        assert!(!opens_directly(&file("x.command", 0o644)));
        assert!(!opens_directly(&file("y.PKG", 0o644)));
        assert!(!opens_directly(&dir.join("A.app")));
        assert!(!opens_directly(&dir), "directory");
        assert!(!opens_directly(&dir.join("missing.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_allows_images_only() {
        let root = Path::new("/r");
        assert_eq!(resolve("prasasti://file/x/y.png", root), Some((PathBuf::from("/x/y.png"), "image/png")));
        assert_eq!(resolve("prasasti://file/x/Y.JPG", root).map(|r| r.1), Some("image/jpeg"));
        assert_eq!(resolve("prasasti://file/etc/passwd", root), None);
        assert_eq!(resolve("prasasti://app/../x", root), None);
        assert_eq!(resolve("prasasti://app/a/%2E%2E/x.js", root), None);
        assert_eq!(resolve("https://example.com/x.png", root), None);
        assert_eq!(
            resolve("prasasti://app/katex/katex.min.js", root),
            Some((PathBuf::from("/r/katex/katex.min.js"), "text/javascript"))
        );
    }

    #[test]
    fn file_url_roundtrips_odd_paths() {
        let dir = Path::new("/tmp/a b/ü#%x");
        let base = base_url(Some(dir));
        assert!(base.starts_with("prasasti://file/") && base.ends_with('/'), "{base}");
        assert_eq!(resolve(&format!("{base}img.png"), Path::new("/r")).map(|r| r.0), Some(dir.join("img.png")));
        assert_eq!(base_url(None), "prasasti://file/");
    }
}
