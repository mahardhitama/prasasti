# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Praśasti for macOS: a native AppKit Markdown writer. Top requirement: typed text is never lost.

## Commands

- `bin/test`: `cargo test --workspace` (core unit and property tests, export unit tests, AppKit app tests).
- Core tests only: `cargo test -p prasasti-core`. One core test: `cargo test -p prasasti-core <name>`.
- App tests: `cargo test -p prasasti --test app`. The binary uses `harness = false` and its own `main` (bottom of `crates/prasasti/tests/app.rs`), so it ignores name filters and always runs the whole list. A new test must be added to the `tests` array in `main`, or it silently never runs.
- `cargo clippy --workspace --all-targets`: must stay error-free.
- `bin/bundle`: release build to `target/bundle.noindex/Prasasti.app` (icon from `resources/prasasti.svg`, fonts copied, ad-hoc signed). `target/Prasasti.app` is a symlink to it. The bundle stays out of Open With because Spotlight skips `.noindex` and the script unregisters it from LaunchServices. Launching it, for example with `open -a`, registers it again until the next `bin/bundle`. To install, copy the real bundle (not the symlink) into `~/Applications`.
- `bin/smoke`: drives the real bundled app through Accessibility/JXA (open, type, autosave, external change, quit). It **takes over the user's desktop** and needs Accessibility permission (exit 2 if missing). It aborts if Praśasti is already running. Don't run it without telling the user.
- Manual checklist and its results: `docs/manual-checks.md`.

## Architecture

Two crates in the workspace:

- `crates/prasasti-core`: pure Rust, no AppKit. Every text position is a **UTF-16 offset** on `&[u16]`, matching NSString. It never panics on any input, and proptest in `tests/props.rs` checks this. It holds Markdown inline and block parsing (`markdown/`), per-paragraph `style_paragraph` with a `BlockState` carried between paragraphs for code fences, search with diacritic folding, edit commands that return an `Edit { range, replacement, selection }`, list indent, word count, zoom steps, UTF-8/LF file rules, and `images` (image destinations, `rebase` of relative paths on Save / Save As).
- `crates/prasasti`: the objc2 / objc2-app-kit app. `main.rs` is a thin shim; `lib.rs::run()` exists so the integration tests can reach the app code.

How the app pieces fit together:

- **`document.rs` (`PrasastiDocument`, an NSDocument subclass)**: owns the whole file lifecycle and nearly all of the data-safety logic.
  - Must be registered as `#[name = "PrasastiDocument"]`, because `packaging/Info.plist` names it as `NSDocumentClass`. A test checks that the plist classes resolve.
  - Autosaves in place. Reads and writes UTF-8 with LF only, and refuses non-UTF-8 files.
  - **Autosave holds** (`HoldReason`: LineEndings for files containing `\r`, ExternalChange, Deleted) are set only through `set_hold`, which also drives the status pill's persistent "Auto-save paused: …" status.
  - While held, autosave never touches the original file. It writes a recovery draft directly with `fs::write` to a fresh name in `~/Library/Autosave Information`, keeping the two newest, because AppKit's autosave-elsewhere fails with error 512 for file-backed documents in a bundled process.
  - When a draft is reopened after a crash, `updateChangeCount:`/`ChangeReadOtherContents` restores the hold.
  - External changes are caught three ways: NSFilePresenter callbacks (which arrive off the main thread and hop to main), `check_disk` comparing disk bytes with `last_known` before every in-place write, and a re-check about 1 s after each save.
  - `breakUndoCoalescing` runs at the start of every save, so typing during or after an autosave counts as a new change.
  - `SHEETS_ENABLED` and `AFTER_SNAPSHOT` are test hooks.
  - Save and Save As rebase local image paths in the text (one undo step, "Update Image Paths") and carry the image files into `<stem>.assets/` via `paste_image::carry`. A failed save undoes the rebase. Autosave never rebases. A document read from disk opens in Preview, an untitled one in Edit.
- **`editor.rs` (`EditorView`, an NSTextView on TextKit 1)**: an explicit storage → layout manager → container chain.
  - `apply_edit` is the single funnel for every programmatic edit (formatting commands, Tab, find/replace): shouldChange → replace → didChangeText → setSelectedRange. That gives one undo step and a restyle.
  - Hidden Markdown markers carry the `PrasastiHidden` attribute. A layout-manager delegate gives them null glyphs.
  - Selection changes restyle the paragraphs the caret left and entered, so markers show only around the caret.
  - `textView:willChangeSelection…` snaps a single-character → step to the start of a hidden run.
- **`content.rs` (`DocumentView`)**: the window's content view. The editor's scroll view fills it under the opaque unified-toolbar titlebar (macOS draws the scroll-edge blur), the find bar sits below the titlebar, the `StatusPill` floats above the bottom toolbar. It owns the mode (`Mode::Edit` / `Mode::Preview`): Preview hides the editor, which stays in the window as the only source of truth, and shows a `PreviewView`. View menu: Preview, Edit, Toggle Preview (⌘E).
- **`preview.rs`**: `page_html` builds the Preview page from the Markdown (pulldown-cmark, math spans, mermaid fences, local images rewritten); `PreviewView` is a WKWebView that serves `prasasti://` (bundled assets from `Contents/Resources/preview`, image files by extension only) under the CSP in `preview::CSP`. Links open outside the web view. A code block's copy button posts a message and the pill flashes "Copied to clipboard".
- **`paste_image.rs`**: `markdown_for_pasteboard` turns pasted image data or image file URLs into `![](…)` links (files go to `<stem>.assets/`, or `~/Library/Application Support/Prasasti/Pasted/<uuid>/` for an untitled document, pruned at launch). Pasteboards with text or a non-image file paste as text. Also `carry` and `assets_dir`.
- **`status.rs` (`StatusPill`)**: glass capsule with the persistent "Auto-save paused: …" text and 2 s flashes ("Copied as HTML.", "Text size N%", save and paste errors). It replaced the footer and its word count.
- **`titlebar.rs`**: the native NSToolbar: centred window title and an appearance button cycling System, Light, Dark (`settings::appearance`, applied to NSApp). **`toolbar.rs`**: the bottom glass Preview | Edit toggle with a sliding thumb. `glass.rs` aliases `NSGlassEffectView`; never substitute `NSVisualEffectView`.
- **`styler.rs`**: applies core `style_paragraph` spans per paragraph, using a `Vec<BlockState>` cache. Restyling keeps going forward while a paragraph's fence state differs from the cache.
- `theme.rs`: Catppuccin Latte (light) and Mocha (dark) palettes; Preview's CSS uses the same values. `settings.rs`: font, zoom, column width and appearance, kept in NSUserDefaults (`editorFont`, `textZoom`, `columnWidth`, `appearance`); tests swap in a separate defaults suite. `find_bar.rs`, `export.rs` (pulldown-cmark → HTML for Copy as HTML and print), `menu.rs`.
- **`util::guarded`**: every ObjC callback that calls into core wraps its body in it, so a panic is caught and never aborts the app while it holds unsaved text.

## Preview resources

`resources/preview/` is copied into the bundle by `bin/bundle`: `preview.css`, `preview.js`, JetBrains Mono (the `fonts` symlink to `resources/fonts`), and vendored KaTeX, Mermaid (`redux-color` / `redux-dark-color` themes, loaded only on pages with a diagram) and highlight.js, with versions in `resources/preview/VERSIONS`. No CDN, nothing fetched at runtime.

## App tests

- Shared helpers live in `tests/ui/mod.rs`: `window_with`, `type_text`, `key`, `shortcut`, `pump`.
  - `ui::key` posts real key events through NSApp's queue.
  - `ui::shortcut` finds the main-menu item bound to a key equivalent (it panics if none exists, which catches a missing binding) and sends its action down the responder chain.
- Prefer driving behaviour through these helpers over calling methods directly.
- `fixtures/kitchen-sink/kitchen-sink.md` exercises every Markdown feature in both modes; tests check its exact content, so copy it before editing. Open it when checking UI changes by hand.
- `main` pins the Aqua appearance, so colour assertions don't depend on the system's Dark mode.
- `build.rs` embeds `crates/prasasti/tests/Info.plist` in the test binary. It gives the tests a bundle id, so saving behaves as it does in the app.
- Tests that touch the clipboard must use `with_clipboard`, which saves and restores every pasteboard item. Tests that create drafts must clean up only the drafts they made.
- If `build.rs` fails with "packaging/Info.plist exists" even though the file is present, the build-script cache has stale paths (another checkout built into this `target/`). `cargo clean -p prasasti` may not be enough. Delete `target/*/build/prasasti-*` and `target/*/.fingerprint/prasasti-*`.
