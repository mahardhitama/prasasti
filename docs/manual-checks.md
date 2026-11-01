# Manual checks

Last run: 2026-11-01, `bin/bundle` build, "Close
windows when quitting an application" on (the default). Driven through System
Events/JXA from the terminal, with screenshots via `screencapture -l<window id>`.
The system prefers tabs, so several documents shared one window as tabs.

Marks: ✅ pass, ❌ fail, **needs user** = a person has to check it.

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
   Dark. The background, text, markers, code panel and footer restyle at once
   (light bg `#ffffff`, dark bg `#101010`). Switch back afterwards.
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
2. `type`: focus the text area, ⌘↓, type `hello`; the text area reads `# Smoke\nhello`.
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
