<p align="center">
  <img src="resources/prasasti.svg" alt="Praśasti icon" width="128">
</p>

# Praśasti

A native macOS Markdown writer. Typed text is never lost.

<p>
  <img src="docs/screenshots/light.png" alt="Praśasti in light mode" width="49%">
  <img src="docs/screenshots/dark.png" alt="Praśasti in dark mode" width="49%">
</p>

## Features

- **Live Markdown styling** in a plain-text editor. Markers stay hidden except around the caret.
- **Preview mode** (⌘E): rendered page with KaTeX math, Mermaid diagrams, syntax-highlighted code with a copy button, footnotes, tables and local images. Everything is bundled; nothing is fetched at runtime.
- **Autosave** in place after typing pauses. Untitled documents are never written under a name you did not choose.
- **Crash recovery** of unsaved text.
- **External change protection**: if the file changes or disappears on disk, or contains CRLF line endings, autosave pauses and your text goes to a recovery draft until you decide.
- **Find and replace** (⌘F, ⌥⌘F) with diacritic-insensitive search.
- **Formatting shortcuts**: bold (⌘B), italic (⌘I), strikethrough (⇧⌘X), link (⌘K), Tab / Shift+Tab list nesting.
- **Image paste**: pasted images and image files land in `<document>.assets/` as `![](…)` links. Save As carries them and rewrites relative paths.
- **Copy as HTML** (⇧⌘C) and print.
- **Text zoom** (⌘+, ⌘−, ⌘0), font choice, and a fixed 80-column layout in JetBrains Mono.
- **Appearance**: System, Light or Dark (Catppuccin Latte and Mocha), with Liquid Glass chrome.
- UTF-8 with LF only; non-UTF-8 files are refused rather than corrupted.

## Architecture

A Rust workspace with two crates:

- **`crates/prasasti-core`**: pure Rust, no AppKit. Markdown inline and block parsing, per-paragraph styling, search with diacritic folding, edit commands, list indent, word count, zoom steps, file rules and image path rebasing. Every text position is a UTF-16 offset, matching `NSString`. It never panics on any input, which property tests check.
- **`crates/prasasti`**: the AppKit app, built on objc2.
  - `document.rs`: an `NSDocument` subclass owning the file lifecycle and data safety: autosave, autosave holds, recovery drafts, external change detection.
  - `editor.rs` and `styler.rs`: an `NSTextView` on TextKit 1. Every programmatic edit goes through one funnel for a single undo step and a restyle; hidden markers get null glyphs.
  - `content.rs`: the window content and the Edit / Preview mode. The editor stays the only source of truth.
  - `preview.rs`: a `WKWebView` serving bundled assets and local images over the `prasasti://` scheme under a strict CSP.
  - `paste_image.rs`, `find_bar.rs`, `export.rs`, `status.rs`, `titlebar.rs`, `toolbar.rs`, `theme.rs`, `settings.rs`, `menu.rs`.
  - Every Objective-C callback into core runs inside a panic guard, so a bug never aborts the app while it holds unsaved text.

## Build

- `bin/bundle`: builds `target/Prasasti.app`.
- `bin/test`: runs every test.
