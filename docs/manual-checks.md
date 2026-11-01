# Manual checks

Last run: 2026-11-01, `bin/bundle` build, "Close
windows when quitting an application" on (the default). Driven through System
Events/JXA from the terminal, with screenshots via `screencapture -l<window id>`.
The system prefers tabs, so several documents shared one window as tabs.

Marks: ✅ pass, ❌ fail, **needs user** = a person has to check it, **pending
(screen locked)** = not yet run, because the Mac's screen was locked when the
preview-and-glass checks were written. Rows below that mention the footer are
history: the footer is gone, and the status pill replaced it.

## Crash recovery

- ✅ **kill -9 restores untitled and saved documents.** Typed into an Untitled
  window and an edited saved `saved.md`. Waited until the autosave landed (about
  15 s), ran `kill -9 <pid>`, relaunched with no arguments. Both came back: Untitled
  read `untitled text`, and `saved.md` read `# Saved\nedited saved`, which was
  also on disk.
- ✅ **⌘Q on an edited untitled window.** A sheet asks "Do you want to keep this
  new document “Untitled”?" with Delete / Cancel / Save. Cancel keeps the window
  and its text. Delete quits and removes the draft. Nothing is lost silently, so
  the `applicationShouldTerminate:` fix (brief Step 2.3) was not needed. After a
  normal quit, no windows are restored on the next launch.
- ✅ **kill -9 while held (CRLF).** `crlf.md` = `# CRLF\r\nline two\r\n`.
  Typed `abc`; the footer read `Auto-save paused: line endings`. The draft
  `crlf.md (Autosaved …).md` appeared about 15 s later holding
  `# CRLF\nline two\nabc`. After `kill -9` and a relaunch with no arguments, the
  window came back with that text and the hold, and the CRLF file was
  byte-identical. (Earlier the draft write failed with
  NSCocoaErrorDomain 512 and the typed text was lost.)
- ✅ **kill -9 while held (external change).** `ext.md` = `# Ext\n`.
  Typed `abc`, wrote `external\n` from outside, chose Keep Mine (footer
  `Auto-save paused: file changed on disk`), typed `def`. The draft held
  `# Ext\nabcdef`. After `kill -9` and a relaunch the window came back with that
  text and the hold; the file still read `external\n`.
- ✅ **⌘Q while held.** Opened the CRLF file above, typed `abc`; drafts
  holding `# CRLF\nline two\nabc` were already present within 1 s of checking.
  ⌘Q (sent after a frontmost check) showed the sheet "Auto-save is paused because
  saving would change the file's line endings. Choose File > Save to keep your
  changes." with OK. The app kept running (checked 3 s later and again after
  ~30 s, when the draft had rotated), the CRLF file was byte-identical, and the
  latest draft still held `abc`. Nothing was lost.

## Preview and glass

Written 2026-11-02. None of these were run: the Mac's screen was
locked, so nothing could be driven or looked at. Setup for all of them:
`bin/bundle && open -a target/Prasasti.app fixtures/kitchen-sink/kitchen-sink.md`.
Don't edit the fixture itself (tests expect its exact content): for the save and
paste checks, copy `fixtures/kitchen-sink` to a temp folder first.

Automated, 2026-11-02: `cargo test --workspace --no-fail-fast` gives 18 app unit,
71 core unit, 12 core property tests ok, and the app integration binary 136
passed, 1 failed: `toolbar_thumb_tracks_mode`, which needs the window on a
visible screen (the thumb sat under the Preview button while the screen was
locked). `cargo clippy --workspace --all-targets` has no errors.

### Window and chrome

- **pending (screen locked)**: **mode on open.** A file opens in Preview. ⌘N
  opens an untitled document in Edit. ⌘E toggles. In Preview, ⌘B changes
  nothing; after ⌘E, typing changes the text. View > Preview and View > Edit
  switch directly.
- **pending (screen locked)**: **glass Preview | Edit toggle (C4).** The capsule
  floats at the bottom. Clicking the other label slides the clear glass thumb
  under it with an animation. ⌘E moves it the same way. The thumb ends centred
  under the selected label. Covered by `toolbar_thumb_tracks_mode` once the
  screen is unlocked.
- **pending (screen locked)**: **native toolbar.** The window title sits centred
  in the titlebar (not at the leading edge) and follows a rename or Save As. At
  the trailing edge a button cycles System, Light, Dark; its icon swaps
  (`circle.lefthalf.filled`, `sun.max`, `moon`) with the symbol replace
  animation, not a jump. Tooltip or VoiceOver reads "Appearance: <name>". The
  choice survives a relaunch.
- **pending (screen locked)**: **status pill.** Open a CRLF file and type: a glass
  pill above the toolbar reads `Auto-save paused: line endings` and stays. ⌘⇧C
  shows `Copied as HTML.`, and zooming shows `Text size N%`, each vanishing after
  about 2 s and then returning to the persistent text if a hold is active. The
  pill is gone when no hold is set.
- **pending (screen locked)**: **column width.** In Edit and in Preview the text
  column is 80 characters wide at a 900 pt and a 1600 pt window. The window
  can't be dragged below the column minimum, and zooming in grows a window that
  is at its minimum.
- **pending (screen locked)**: **autosave in Preview.** Type in Edit, press ⌘E,
  wait about 15 s: the file on disk has the text.

### Palettes and appearance

- **pending (screen locked)**: **Catppuccin.** Light is Latte (background
  `#eff1f5`, text `#4c4f69`, accent `#1e66f5`); Dark is Mocha (`#1e1e2e`,
  `#cdd6f4`, `#89b4fa`). Editor and Preview match in each, and the editor's
  code panel, selection, find highlights and the pill follow.
- **needs user**: **live switch.** With a document open in each mode, flip System
  Settings > Appearance, then use the toolbar button for Light and Dark. Both
  the editor and Preview restyle at once, with no relaunch.

### Preview content (the kitchen-sink fixture)

- **pending (screen locked)**: **math.** The inline `e^{iπ}+1=0` sits in its
  sentence. The two display blocks are centred. The invalid
  `$$\frac{1}{$$` block shows an error box and nothing else on the page breaks.
- **pending (screen locked)**: **Mermaid.** The flowchart, sequence and state
  diagrams render. The broken diagram shows an error box alone. Repeat in Dark.
- **pending (screen locked)**: **code blocks.** All use JetBrains Mono. rust,
  swift, js, json, bash, python and html show highlight.js colours from the
  Catppuccin mapping; `klingon`, `text` and the no-language fence stay plain.
  Every block has line numbers, and the number column widens for more digits.
  The long `text` line scrolls horizontally instead of wrapping. In Dark the
  colours switch palette.
- **pending (screen locked)**: **copy button.** Each code block has one in its
  corner. It does not scroll with a long line and is not part of a selection of
  the code. Clicking copies exactly the code (paste it into a scratch document
  to check), shows the pill `Copied to clipboard` for about 2 s, and swaps the
  icon to a checkmark for about 1.5 s, then back.
- **pending (screen locked)**: **images.** Relative PNG, JPEG in a subfolder, SVG
  and the file name with a space all show. The data URI shows a dot. `Missing`
  shows a broken image, not a blank page. In a copy of the fixture moved to
  another folder the relative images still show.
- **needs user**: **remote image.** The Rust logo loads (needs the network).
- **pending (screen locked)**: **raw HTML.** The inline SVG shows an orange bar,
  the `<details>` block expands, and `<kbd>` renders as keys.
- **pending (screen locked)**: **links.** Clicking https links opens the default
  browser and leaves Preview on the page. `#headings` scrolls inside Preview.
  `mailto:` opens the mail app. A link to a local file opens it in its default
  app (or reveals it in Finder if it is executable), never inside the web view.
- **pending (screen locked)**: **script isolation.** The `<script>` and
  `<img onerror>` probes at the end of Raw HTML have no visible effect and no
  alert appears. Automated part: `page_has_csp_and_column`.
- **pending (screen locked)**: **non-ASCII.** Japanese, Chinese and Arabic
  paragraphs wrap at the column, the Arabic one right to left; emoji and
  combining marks render.

### Images in the document

- **pending (screen locked)**: **rebase on first save.** In an untitled document
  type `![](/abs/dir/img.png)` (a real folder and file), ⌘S into `/abs/dir`: the
  editor and the file both read `![](img.png)`. ⌘Z undoes it in one step named
  "Update Image Paths".
- **pending (screen locked)**: **rebase and carry on Save As.** Open
  `kitchen-sink.md` from a temp copy, ⌥⇧⌘S into another folder. Relative image
  links are rewritten, the images are copied into `<stem>.assets/` beside the
  new file and still show in Preview, and the original folder is untouched. An
  autosave alone never rewrites links.
- **pending (screen locked)**: **failed copy.** If an image can't be copied (for
  example a read-only target), the link keeps its old path and the pill shows
  `Couldn't copy <name>: …`; the save itself still succeeds.

### Pasting images

- **needs user**: **screenshot.** In an untitled document press ⌃⇧⌘4, drag a
  region, then ⌘V. A link `![](/…/Pasted/<id>/paste-YYYYMMDD-HHMMSS.png)` appears
  and Preview shows the image. ⌘S into a folder copies the file into
  `<stem>.assets/` and rewrites the link to the relative path.
- **needs user**: **saved document.** The same paste into a saved document writes
  `<stem>.assets/paste-….png` and inserts the relative link. A second paste in
  the same second gets a `-2` suffix. ⌘Z removes the link and keeps the file.
- **needs user**: **TIFF.** Copy an image from Preview.app or Pixelmator (TIFF on
  the pasteboard): the pasted file is a `.png`.
- **needs user**: **Finder image file.** Copy a `.png` in Finder, ⌘V: a link to
  its own path (relative to a saved document), and no copy is made.
- **needs user**: **non-image Finder file.** Copy a `.pdf` or `.txt` in Finder,
  ⌘V: it pastes as text, and no image link (the file's icon is not pasted).
- **needs user**: **Office text.** Copy a cell or a paragraph in Word or Excel
  (text plus a rendering): only the text pastes.
- **needs user**: **plain paste.** ⌥⇧⌘V with an image on the pasteboard pastes
  text only, or nothing if there is no text.
- **needs user**: **save failure.** Paste an image into a document in a
  read-only folder: the pill shows `Couldn't save pasted image: …` and the text
  is unchanged.

### Smoke

- **pending (screen locked)**: **`bin/smoke` with ⌘E.** It now presses ⌘E after
  `open` and before focusing the text area, because `note.md` opens in Preview.
  Not run: it takes over the desktop. Expect every check `ok` and exit 0.

## Checklist

- ✅ **Print preview renders headings, bold and code.** Used ⌘P, then PDF > Save as
  PDF. The heading is large and bold, `bold` is bold, and code and the fence are
  monospaced. The printed text uses the editor font (JetBrains
  Mono by default; before, the HTML importer fell back to Times) and starts at
  the top of the page (before, it was vertically centred). Covered by the app
  test `print_uses_editor_font_top_aligned`.
- ✅ **⌘T opens the Fonts panel, and picking a font changes every window.**
  Choosing Andale Mono changed both open tabs (read with the AX `AXFont`
  attribute) and wrote `editorFont`.
- ✅ **The font survives a relaunch.** After ⌘Q and a relaunch, the editor was
  still Andale Mono.
- **needs user**: **switching appearance while a window is open restyles it.**
  See below.
- ✅ **Open With opens a `.md` file.** `open -a target/Prasasti.app file.md`
  worked both at launch and with the app already running.
- **needs user**: **⌘-scroll zooms.** See below.
- ✅ **⌃⌘F toggles full screen.** `AXFullScreen` turned on, then off again.
- ✅ **The window frame is remembered.** Moved and resized to 400,300 800×600,
  quit and relaunched; the frame was the same.
- ✅ **⌘N / ⌘O / ⌘S / ⇧⌘S.** ⌘N opens Untitled. ⌘O shows the Open panel and opens
  the chosen file. ⌘S writes to disk at once. ⇧⌘S is **Duplicate** (Save As… is
  ⌥⇧⌘S, as with autosave-in-place); ⌘S on the copy shows the save sheet and wrote
  `dup.md`.
- ✅ **An external edit triggers the sheet.** With local edits, overwriting the
  file shows "The file changed on disk." with Keep Mine / Reload. `bin/smoke`
  also covers Reload.

## From reviews

- ✅ **⌘⇧= zooms (real key).** The physical key (key code 24 with ⇧⌘) went
  13 → 14.3 pt; ⌘- went 13 → 11.7, and ⌘0 reset it. System Events `keystroke "="`
  with ⇧⌘ does nothing, because it sends a different character; real keyboards
  send the key code.
- ✅ **Footer in Dark mode.** Launched with `--args -AppleInterfaceStyle Dark`.
  The footer reads `3 Words  Auto-save paused: line endings` with the Font
  button, in light text on a dark bar, and is readable.
- **needs user**: **monospaced fallback when JetBrains Mono is missing.** It can't be
  forced without uninstalling the font.
- **needs user**: **typing feel with the find bar open on a 2 MB document.**
  Automated part: on a 1.97 MB document (340,000 words), the find bar opened
  and highlighted every `words` match.
- ✅ **Move to Trash while open triggers the Deleted sheet.** "The file was
  deleted or moved to the Trash." with Close / Save.
- **needs user**: **IME marked text while an autosave fires.** See below.

## Needs the user

1. **⌘-scroll zoom.** System Events can't synthesise scroll. Open a document,
   hold ⌘ and scroll up on a mouse or trackpad: the text grows step by step.
   Scroll down: it shrinks. A slow trackpad scroll changes at most one step per
   notch.
2. **Live appearance switch.** Open a document with some Markdown (a heading,
   `code`, a link), then switch System Settings > Appearance between Light and
   Dark. The background, text, markers, code panel and status pill restyle at once
   (Catppuccin Latte bg `#eff1f5`, Mocha bg `#1e1e2e`). Switch back afterwards.
3. **Plex fallback.** On a Mac without JetBrains Mono available (for example a
   build without `resources/fonts`), open a document: the editor uses a
   monospaced system font (Menlo or SF Mono), not a proportional one.
4. **Typing feel, 2 MB with the find bar.** Generate the file with
   `python3 -c "p='## S {}\n\nSome **bold** \`code\` [l](x) words words words.\n\n'; open('/tmp/big.md','w').write(''.join(p.format(i) for i in range(30000))[:2000000])"`,
   then `open -a target/Prasasti.app /tmp/big.md`. Press ⌘F, type `words`, click
   in the text and type fast for 10 s, in the middle and at the end. Keystrokes
   should feel instant, with no visible lag or stutter.
5. **IME during autosave.** Turn on a Japanese (Kana or Romaji) or Pinyin input
   source. Open a saved file, type a character and wait about 12 s, then start
   composing (marked, underlined text) and keep composing past the 15 s autosave.
   The composition must not be committed, broken or duplicated. After
   committing, the file on disk holds the committed text after the next
   autosave.

## Automated smoke

`bin/smoke` builds `target/Prasasti.app` with `bin/bundle` and drives it through
System Events (JavaScript for Automation). It takes about 25 s.

```sh
bin/smoke
```

**Permission:** the terminal running it needs Accessibility access
(System Settings > Privacy & Security > Accessibility). Without it the script
prints how to grant it and exits 2. It changes no settings itself.

**Covers**, each printed as `smoke <name> ... ok|FAILED`:

1. `open`: `open -a` a temp `note.md` (`# Smoke`); a window titled `note.md` appears within 5 s.
   If an alert opens instead, its text is printed.
2. `type`: press ⌘E (a file opens in Preview), focus the text area, ⌘↓, type `hello`; the text area reads `# Smoke\nhello`.
3. `autosave`: `hello` reaches the file within 30 s (AppKit's in-place autosave lands ~15 s after an edit).
4. `external-change`: type `x`, overwrite the file with `external`; a sheet with
   `Reload` and `Keep Mine` appears within 5 s; after Reload the text area reads `external\n`.
5. `quit`: ⌘Q quits the clean document without a prompt within 5 s.

Exit 0 when every check passes, 1 on the first failure (or when Praśasti is
already running), 2 without Accessibility permission.

**Safety:** it refuses to run while another Praśasti is open, and only sends
keys after System Events reports Praśasti frontmost (otherwise that check fails
with no keys sent). It works in a temp dir, launches with
`-ApplePersistenceIgnoreState YES` so earlier windows are not restored and your
saved state is untouched, and on exit quits its instance and removes the temp
dir and the `note…` recovery drafts it created in `~/Library/Autosave Information`.
Don't type in other apps while it runs: focus switches to Praśasti.
