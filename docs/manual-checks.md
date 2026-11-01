# Manual checks

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
