# Leapp RS F0: carry-forward list

F0 (tracer bullet) is complete and merged to `master` on 2026-11-24. This file keeps
the deferred findings from the per-task reviews and the final whole-branch review, so that the
plans for later features can include them. Each later plan must read its section below.

## F1 (IAM user edit/delete, MFA, refresh)

F1 is complete and merged to `master` on 2026-11-25 (merge `e8e60113`). All items are closed, or
moved (see the last two items).

- Done (`c69761cf`, test `a_start_that_waits_on_aws_at_shutdown_leaves_no_backup`): keychain backup
  orphan when you quit during a start. A start checks `closing` before the backup write and
  before the STS call.
- Done (`c69761cf`, test `a_start_after_shutdown_makes_no_sts_call`): no STS call after shutdown.
- Done (`c69761cf`): `closing` is a `bool` in the `Mutex<State>`, beside the workspace.
- Done (`c69761cf`, test `a_stop_does_not_hold_the_lock_while_the_keychain_reads`): `stop` and
  `clean_all` read the keychain without the workspace lock.
- Done (`9f5fbe2f`, `30bea513`): the form submit runs `create_iam_user` in `spawn_blocking`, off
  the render thread, and reports `Msg::FormDone`.
- Done (`f016249b`, `8f131d86`, `d753febc`): the token cache has a margin
  (`TOKEN_REUSE_MARGIN`, 1200 s), the expiry path has tests (`tests/token_cache.rs`), and an edit of
  the keys or the MFA device deletes the cached token.
- Done (`c69761cf`, test `an_aws_error_with_an_empty_message_shows_the_full_error`): `aws_message`
  and `Some("")`.
- Done (`c69761cf`, test `a_stop_with_a_missing_backup_says_so`): a missing backup gives
  `Error::BackupMissing`.
- Done (`c69761cf`, test `a_failed_second_secret_write_leaves_no_session_and_no_keys`): the second
  `SecretStore::set` failure path.
- Done (`a0cda4fc`): the `Ui` keeps a `starting` set, so a workspace read keeps a starting session
  `Pending`.
- Done (`c69761cf`): `stop` reads its target and the `Pending` check under one lock
  (`stop_target`).
- Done (`9f5fbe2f`, tests `each_input_error_shows_under_its_field`,
  `typing_in_a_field_clears_its_error`): field errors clear on edit, and `FieldErrors` matches the
  core `Field` enum, not strings.
- Done (`511f6327`, F0): the region field is a `Combobox`; its popup is as wide as its trigger. While
  it is open, it covers the dialog buttons (a normal popup; a pick closes it).
- Done (`a0cda4fc`): the empty state has an `Add session` button (`btn-empty-add-session`).
- Done (`5628104e`): the question buttons are disabled during the arming delay.
- Done (`9f5fbe2f`, `30bea513`, test `the_eye_shows_the_secret`): eye toggle on the secret field
  (ruling R10), with the MFA device field.
- Done (`9f5fbe2f`, test `the_dialog_fits_the_smallest_window`): the Create dialog at 1032x480.
- Done (Task 9, test file `crates/leapp-app/tests/refresh.rs`): the app refresh loop
  (`spawn_refresh_loop`, `REFRESH_TICK`).
- Done (`cb20492a`, tests `a_start_that_panics_leaves_the_session_inactive` and
  `the_loop_runs_again_after_a_tick_panics`): a panic in a start or a refresh no longer leaves the
  session `Pending` (`PendingGuard`).
- Moved to "Small items": the region search test assumes that "eu-w" matches only `eu-west-*`.
- Moved to "Small items": the region option list is rebuilt on each frame while the dialog is open.

## F2 (named profiles)

F2 and F3 are done on branch `leapp-rs-f2-f3` (not yet merged).

- Done (`57b57606`, tests `the_profile_field_lists_the_profiles`, `a_new_profile_name_can_be_typed`,
  `an_existing_name_offers_no_create_option` in `tests/profiles_field.rs`): the Named Profile field
  is a select with a create option.
- Done (`332d10cf`, `aa331871`, tests `the_default_region_is_set_in_settings` in
  `tests/settings.rs` and `the_form_preselects_the_default_region` in `tests/profiles_field.rs`):
  the default region is a setting; the forms use it.

Deferred by the F2 and F3 reviews and the final review (open unless marked done):

- Profiles: a profile named `__new__` shares the key `sel-profile-option-__new__` with the create
  option. No test for an unchanged-profile edit, or for "default " with no create option.
- Done (final fix wave, tests `a_late_result_of_an_old_rename_does_not_change_a_new_rename` and
  `a_rename_that_must_wait_keeps_the_typed_name` in `tests/settings.rs`): each rename has a token,
  so a stale `RenameDone` does not change a new rename; the rename field has no `expect`; a
  `ProfilePending` result keeps the rename field open with the typed name and shows the error
  under it (final review I1, M1).
- Profiles: a profile delete can be submitted twice, and the second delete shows
  `ProfileNotFound` with the profile UUID in the toast (final review M4: use the name, or ignore
  `ProfileNotFound` after a delete). A failed `update_settings` leaves the select on the unsaved
  value; the tabs have no selected state for accessibility.
- Profiles: `update_settings` and `create_profile` run on the render thread and take the core
  lock. With the keychain write under the lock (see F3), a slow keychain call freezes the UI also
  through them (final review M3).
- Profiles (core): `delete_profile` refuses when a default session is `Pending`. A user stop during
  a change is undone (same as `update_iam_user`). The chmod tests fail as root.
- Change region and profile dialogs: Cancel and close are locked while a change saves (a hung AWS
  call keeps the dialog up until the STS timeout, 10 s). No test for a change whose restart asks
  MFA or overwrite. The `InvalidInput` match is not field-specific. Unused `_tx` in `save`.
- Core (Task 1): T1-m2, the F23-R3 no-op check and the target-profile `Pending` check of
  `change_profile` run in their own critical section before `change_sessions` (a race gives a
  `ProfilePending` restart error with the change saved). T1-m6, T1-m7, T1-m8: the ledger has no
  text for them, and the final review found no defect; read the Task 1 review again before F4.
- Core (Task 2): T2-m2, T2-m3, T2-m4, T2-m6 to T2-m9: the ledger has no text. The implementer's
  concerns: no test for the stop-failure path of `change_sessions` or for
  `no_other_session_runs_on`, and a second overwrite question after a failed restart.

## F3 (IAM Role Chained)

F3 is done on branch `leapp-rs-f2-f3` (commits `6c6e9e8c`, `8bd44738`, `4158f4b9`, `e7f2b3bb`,
`4d555fdf`, `ee9ef0cf`). Tests: `crates/leapp-core/tests/chained.rs`,
`crates/leapp-app/tests/chained.rs`, `crates/leapp-app/tests/row_menu.rs`.

Deferred by the F3 reviews and the final review (open unless marked done):

- Done (final fix wave, test `a_parent_token_of_keys_from_a_failed_edit_is_not_kept`): an edit of
  the keys or the MFA device bumps the key generation also when its save fails, so a parent token
  of the rejected keys is not kept or reused (F1-R4, final review I3).
- Done (final fix wave, tests `a_create_under_a_chain_that_a_delete_holds_is_refused` and
  `an_edit_to_a_parent_that_a_delete_holds_is_refused`): `check_parent` refuses a parent that an
  `Editing` reservation holds with `ProfilePending`, so a delete cannot remove a session that was
  just created or moved under its chain (final review I2).
- Done (final fix wave): `CLAUDE.md` has the real change-dialog keys, the option keys
  (`<select key>-option-<value>`, `sel-parent-option-<session id>`), the row menu labels, the
  true statement about the `Editing` reservation, and the scenes line wrapped (T11-1 to T11-3,
  final review I4, M5).
- Done (final fix wave): `#[expect(clippy::too_many_arguments, reason = ...)]` on `text_field` in
  `chained_dialog.rs` (T9-m3, final review M2).
- Core: two parallel fresh parent tokens can split the keychain token and its expiry. The
  MFA-only test also changes the region.
- Core: `save_parent_token` writes to the keychain under the core state `Mutex`. A blocking
  keychain prompt would freeze the UI, which reads the workspace each frame. Option: a per-parent
  write lock. F4 must decide this before it copies the pattern (see the `CLAUDE.md` gotcha).
- Core: no test for the cascade stop order.
- Done (test `a_parent_token_of_keys_from_a_failed_key_write_is_not_kept`): if the second key
  write of `update_iam_user` failed, `KeyChange::write` undid the first one but did not bump the
  key generation, so a chained start that read the first new key kept a token of it (F1-R4). Now
  the key generation goes up in one place, after the keychain and the workspace hold their final
  keys, on every path (success, failed write, failed save after the undo).
- Core (Task 3): T3-m1, T3-m3, T3-m4, T3-m5: the ledger has no text, and the final review found no
  defect in `chained.rs`. Read the Task 3 review again before F4.
- Core (Task 4): T4-m5 (ruled, no change): the missing-parent message names the session whose
  parent is missing, not the session that was started. It is accurate for a deep chain.
- Core: `update_iam_role_chained` copies about 40 lines of the `update_iam_user` skeleton. The
  `From<IamRoleChainedUpdate>` impl, the `Editing` doc comment and the duplicated `modes_are_ignored`
  need a rust-review clean-up.
- App: label case differs between forms ("Named profile" and "Session alias" in the chained form;
  "Named Profile" and "Session Alias" in the IAM User form). Parent options with the same name
  cannot be told apart. No app test for `err-parent`, `err-chained-name`, `err-chained-form`.
- App: `purple()` is in `chained_dialog.rs`, not next to `orange()`. Unused `Tx` in `submit`.
  `request()` checks the parent again. `ChainedForm::is_open` has no doc.
- App: the delete count is frozen when the confirm opens (compute it in `build` from
  `ws.descendants`). A chained `open_edit` with an unknown profile gives an empty profile. The
  `chained()` test helper is copied in two test files. No stale-token test for a chained create or
  edit.

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
- One MFA question blocks the whole refresh tick: the other due sessions are not refreshed until
  the user answers. For F8 or F3: refresh the MFA sessions last, or give the question a timeout.

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
- The region search test assumes that "eu-w" matches only `eu-west-*` (rune-kit fuzzy match).
- The region option list is rebuilt on each frame while the dialog is open (about 30 items).
- A stop that a refresh refuses (the refresh holds the session `Pending`) says "Another session
  with the same named profile is starting". It should say that the session is being refreshed.
- A delete that an edit's reservation refuses also says "Another session with the same named
  profile is starting" (`editing_error()`). It should say that the session is being edited.
- A panic during a refresh sets the session `Inactive` in memory only (`PendingGuard` does no I/O).
  The managed entry and our section in `~/.aws/credentials` stay until `shutdown`, `startup` or a
  new start and stop of that session.

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
