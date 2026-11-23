# Leapp RS F0: carry-forward list

F0 (tracer bullet) is complete on branch `leapp-rs` (commits 6b4cff03..c4176fe4). This file keeps
the deferred findings from the per-task reviews and the final whole-branch review, so that the
plans for later features can include them. Each later plan must read its section below.

## F1 (IAM user edit/delete, MFA, refresh)

- Keychain backup orphan when you quit during a start. `activate` writes the backup before the
  STS call, with no `closing` check. At a real quit the runtime drops the start, so
  `aws-profile-backup-<P>` stays. It equals the user's section, so no data is lost. If the user
  edits `[P]` first, the next start fails with `BackupConflict`. Fix: check `closing` before the
  backup write and before the STS call, or let `clean_all` remove orphan backups that are equal to
  the file section.
- An STS call is still made after shutdown for a start that already passed the `Pending` check.
- Put `closing` as a `bool` inside the mutex-guarded state, not as an `AtomicBool` beside the
  `Mutex` (the type then enforces "only under the lock").
- Move keychain reads (`stop`, `clean_all`) out of the workspace lock before the refresh loop adds
  a second writer. A keychain prompt under the lock freezes every render-thread `workspace()` call.
- `create_iam_user` runs on the render thread (sync keychain). Move it to `Ui::run`.
- Token cache: no margin before expiry; the expiry path has no test; the cache is not dropped when
  the base keys change (edit must delete it).
- `aws_message` passes `Some("")` through.
- `had_original` with a missing backup removes the section silently. Show an error instead.
- The second `SecretStore::set` failure path has no test.
- Local `Pending` can flip to inactive on an early refresh. Add a "starting" set that `refresh`
  applies again.
- `to_stop` and the `Pending` check use two lock acquisitions (worst case: a stale "Active" label).
- Create dialog: field errors clear only on submit; no tests for the `err-access-key`,
  `err-secret-key` and `err-form` mapping; match fields with an enum, not strings.
- Region Select popup is narrower than its trigger and hides the Cancel button.
- Empty state has no "Add session" button.
- Overwrite dialog: the buttons do not look disabled during the 0.5 s arming delay.
- Eye toggle on the secret field (spec 14.5), together with the MFA field (ruling R10).

## F2 (named profiles)

- The Named Profile field is a text field. Replace it with a select.
- Hard-coded default region `us-east-1` moves to settings.

## F4 (AWS SSO)

- Empty state: "Add AWS SSO integration" button.

## F8 (menu bar, notifications, logging)

- Logging for dropped secondary errors (ruling R8), e.g. a failed backup delete after save.
- `Leapp::open` failure writes to stderr only. A Finder launch shows nothing.
- A keychain prompt during `shutdown` at quit freezes the window (`on_close` uses `block_on` on
  the UI thread).
- `clean_all` is all-or-nothing. One keychain error at quit cleans no profile. Consider best-effort
  cleanup per profile, and show the error before quit.
- `Error::Io` has no path, so the toast can omit the file name.
- AWS error text in the toast can be long and technical.

## F9 (packaging, hardening)

- fsync the folder after a rename.
- Zeroize the secret copies.
- chmod tests fail when they run as root. Timing-based tests (50 ms / 300 ms) can be flaky on slow CI.
- `persist()` replaces a symlinked `~/.aws/credentials` with a regular file.

## Small items with no feature

- Mark `Error` as `#[non_exhaustive]`. Replace the private `bool` parameters
  (`clean_all(closing)`, `activate(.., consent_carried)`) with enums.
- Use `#[expect]` instead of `#[allow]`. Remove `#![allow(dead_code)]` from `tests/common`. Add
  `#[must_use]` to `install()`.
- Some `pub` accessors exist only for tests (`pending_confirm_count` and the create-form getters).
- `wake_after` is called on every poll while the toast shows. A double-click on an Active status
  sends up to 3 stops; the later stops do nothing.
- Add one line to `CLAUDE.md`: `--native` with `--script` is not headless, so it uses the real home
  and the real keychain.
- No test covers the `is_headless()` call site, or the wake at the end of the arming delay.
- `fake-aws` does not escape XML in error messages.

## Decisions made without the owner (rulings)

| # | Decision | Cost if wrong |
|---|---|---|
| R1 | `AppHarness` has a `pub bridge` so tests can trigger a refresh | One field rename |
| R2 | `spec().init` builds the `Ui` from a `OnceLock` that `install()` fills | Small refactor |
| R3 | A headless run (`--snap`, `--tree`, `--script`, …) uses a temp home and `MemoryStore` | Minor |
| R4 | One new commit per task and per fix (no amend, no squash) | More commits |
| R5 | Every task review also runs rust-review | Longer reviews |
| R6 | `stop` on a `Pending` session returns `ProfilePending` and changes nothing | One more UI error path |
| R7 | Blocking I/O in async fns is documented, not moved to `spawn_blocking` | A keychain prompt blocks one tokio worker |
| R8 | Logging of dropped errors waits for F8 | Harder diagnosis |
| R9 | New `Error::BackupConflict`; a stale backup equal to the file section is reused | One more variant |
| R10 | The eye toggle on the secret field waits for F1 | Small UI addition later |
| R11 | Overwrite questions are queued and shown one at a time; never an implicit "no" | Small UI state change |
| I2 | Switching sessions on the same profile keeps the overwrite consent (spec 9.3 step 5) | One prompt path to restore |
| I3 | Overwrite answers are ignored for 0.5 s after a question shows | A slow user waits half a second |
| — | Empty Role column and no Status header are kept (they match spec 14.1) | Small table change |
