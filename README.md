# ltui

A keyboard-driven macOS launchd job manager, built with Rust and Ratatui.

```sh
cargo run --release
# Or install the binary:
cargo install --path .
ltui
```

Use `ltui --demo` to explore sample jobs without changing any files or services.

## What it does

- Browse user agents, shared agents, daemons, Apple jobs, and session/system services, including jobs without a plist in the standard directories.
- Show running PIDs, loaded/idle/unloaded state and enabled/disabled policy. Refresh automatically every five seconds or with `R`.
- Create and delete jobs; load, unload, enable, disable, run now, restart, reload, and send TERM/KILL/HUP signals.
- Edit the Label, executable, argument vector, working directory, output paths, RunAtLoad, KeepAlive, start interval, calendar schedules, and environment variables in a form. Keys outside the form are preserved, including throttle, daemon user/group, and dictionary-valued KeepAlive conditions.
- Edit the entire XML plist using `$EDITOR` (then `$VISUAL`, then `vi`), including binary plists converted to XML for editing. For GUI editors, use a waiting option, such as `EDITOR='code --wait'`.
- View the selected job’s plist in the right panel. Open configured stdout/stderr log files from the actions menu using `$PAGER` (default: `less`).

## Keys

| Key | Action |
| --- | --- |
| `↑`/`↓`, `j`/`k`, `Ctrl-N`/`Ctrl-P` | Select job (Ctrl-N down, Ctrl-P up) |
| `Tab` / `Shift-Tab` | Next / previous scope |
| `/` | Search labels, commands, policy, and runtime state |
| `Esc` | Clear search / cancel dialog |
| `Space` | Open actions: edit, edit with `$EDITOR`, stdout/stderr logs, and launchctl |
| `Enter` | Open the selected plist in `$EDITOR` |
| `Ctrl-U` / `Ctrl-D` | Previous / next half page of jobs |
| `PgUp` / `PgDn` | Previous / next full page of jobs |
| `J` / `K` | Scroll the plist panel down / up by half its visible height |
| `g` / `G` | First / last job |
| `n` | Create job in this scope |
| `e` | Edit common options |
| `l` / `u` | Load / unload |
| `s` / `r` | Run now / restart |
| `t` | Toggle enabled / disabled |
| `x` | Unload and delete plist |
| `R` | Refresh |
| `?` | Help and inventory warnings |
| `q` / `Ctrl-C` | Quit |

Outside forms, `←`, `→`, `E`, `a`, `p`, and `o` have no action. The actions menu has a file section followed by launchctl operations. Use `↑`/`↓`, `j`/`k`, or `Ctrl-N`/`Ctrl-P` to choose an item and Enter to select it. Quit the log pager to return to ltui.

In forms, use `Tab`/`Ctrl-N` for the next field or `Shift-Tab`/`Ctrl-P` for the previous field, including from multiline values. Multiline values expand and scroll to keep the cursor visible. `↑`/`↓` move between lines, or between fields at the first/last line; `←`/`→`, `Home`/`End`, Backspace, and Delete edit the current line. `Ctrl-Home`/`Ctrl-End` jump to the start/end of the whole field. Enter inserts a newline in arguments, calendar schedules, and environment variables, and pasting preserves their newlines. Use `Ctrl-U` to clear a field, `Ctrl-S` to save, and `Esc` to cancel. Arguments use shell quoting to separate words. No shell expansion happens: to run shell syntax, specify `/bin/sh -c 'your command'` explicitly.

Calendar schedules use `Hour=23 Minute=31` for daily at 23:31. Omit a key to match any value. Available keys are `Minute` (0–59), `Hour` (0–23), `Day` (1–31), `Month` (1–12), and `Weekday` (0–7; 0 and 7 are Sunday). Put each schedule on its own line to run at multiple times. `Day` and `Weekday` match either when both are specified. `*` means every minute; leaving the field blank removes the schedule. Existing dictionary and array schedules retain their plist structure.

Environment variables use `KEY=value` assignments separated by spaces or newlines, for example:

```text
PATH=/opt/homebrew/bin:/usr/bin:/bin
MODE=dev
MESSAGE='hello world'
EMPTY=''
```

Quote values containing spaces. Do not include `export` or commas between assignments. Values are literal: `$HOME`, `~`, and command substitutions are not expanded. Leaving the field blank clears all environment variables. The form shows these examples when the calendar or environment field is focused.

## launchd behavior

Enabled/disabled and loaded/unloaded are independent. **Enable** allows future loads but does not load a job. **Disable** changes the persistent launchctl override but does not stop a running process. **Load** uses `bootstrap`; **Unload** uses `bootout` for the selected service. TERM/KILL may be followed by a restart if KeepAlive or another launch condition applies.

New plists are saved unloaded. Saving an existing loaded job offers to reload it; decline to apply the changes later. Reload unloads first and then bootstraps the updated plist. If bootstrap fails, the error explains that the job is now unloaded. Disabled jobs must be enabled before reloading.

The job's name is its `Label`. Changing it keeps the original filename. Unload before renaming a loaded job so its old service is not left running. Duplicate Labels are rejected when loading or saving.

Edits use a private temporary copy. The original is replaced atomically after plist parsing and validation of common launchd keys. Disk changes made during editing are detected. Failed external edits retain a recovery copy and display its path. Existing permissions and ownership are preserved; symlinked plists update the target while keeping the symlink. Deleting a symlinked job removes the symlink, leaving its target file intact. Delete requires confirmation and unloads the job before removing the plist; executables and log files are kept.

## Scopes and permissions

| Scope | Plists / services | Domain |
| --- | --- | --- |
| `user` | `~/Library/LaunchAgents` | `gui/UID` |
| `agents` | `/Library/LaunchAgents` | `gui/UID` |
| `daemons` | `/Library/LaunchDaemons` | `system` |
| `apple-agents` | `/System/Library/LaunchAgents` | `gui/UID` |
| `apple-daemons` | `/System/Library/LaunchDaemons` | `system` |
| `session` | Agent plists plus services registered in the agent domain | `gui/UID` |
| `system` | Daemon plists plus registered system services | `system` |

Creating from Session uses your user LaunchAgents directory; creating from System uses `/Library/LaunchDaemons`. Apple plist editing/deletion is read-only. macOS may also protect Apple services from launchctl changes.

```sh
ltui --scope daemons
sudo ltui --scope daemons        # modify system jobs / root-owned plists
ltui --domain user/501           # target a headless user domain
ltui --list                     # read-only text inventory
ltui --list --scope session
ltui --help
```

By default, sudo retains the invoking user's GUI domain and home directory. `--domain gui/UID` or `--domain user/UID` overrides the agent target; it does not change the plist directories. Daemons always target `system`. The app does not invoke sudo itself; permission failures appear in the UI.

Runtime discovery parses `launchctl print` and `print-disabled`. Apple does not guarantee their output format, so an unrecognized or failed query produces unknown status and a warning. Runtime-only services may have no editable plist; when available, their plist path is resolved on selection. Some SMAppService/bundle-managed jobs must be configured through their owning application.

The launchd semantics follow the installed `man launchctl`, `man launchd.plist`, and [Apple's launchd guide](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html).

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
# macOS integration smoke test, using only temporary plists and read-only launchctl queries:
cargo build
python3 tests/terminal_smoke.py
```

Tests cover status parsing, disable overrides, command targeting, failure handling, conflict detection, advanced-key preservation, atomic/symlink saves, text editing, confirmation dialogs, and rendering at different terminal sizes. Integration tests never load or start a job.
