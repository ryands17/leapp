# Leapp RS F1 (IAM User complete) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An IAM User session that is complete: MFA, the token cache with expiry, edit, delete,
the refresh loop, the row menu (Start/Stop, Edit, Delete, Copy account number), plus the F1 items
that the F0 reviews deferred.

**Architecture:** All AWS, file and keychain work stays in `leapp-core` behind the `Leapp` facade.
Core gets `update_iam_user`, `delete_session` and `refresh_tick`; it has no timer. The app runs
`refresh_tick` every 10 s on its tokio runtime. Core asks for the MFA code through a new `Prompter`
method; the app answers it with a dialog that shares the question queue of the overwrite dialog.
Every core call from the UI runs off the render thread (`Ui::run`).

**Tech Stack:** Rust 2024 (toolchain 1.100), Rune + rune-kit at rev
`3d4e2dfcecb71890f7f4d6167e82fe6db9dbb695` (`Menu`, `context_menu`, `Dialog`, `Combobox`,
`TextInput`, `Tx::copy`), tokio, aws-sdk-sts, keyring, axum (fake-aws).

**Spec:** `docs/superpowers/specs/2026-11-23-leapp-rs-design.md`. Read sections 5, 9.3, 9.4, 10,
11.1, 14.1, 14.3, 14.5, 14.7, 15 and 16 (F1). Also read the F1 section and the rulings table of
`docs/superpowers/plans/2026-11-24-leapp-rs-f0-carry-forward.md`. Leapp reference:
`packages/core/src/services/session/aws/aws-iam-user-service.ts` (update, MFA, caller identity),
`packages/core/src/services/rotation-service.ts`, `packages/core/src/models/session.ts:31`.

## Global Constraints

- Run all commands in `leapp-rs/`. Done means `cargo test --workspace` passes,
  `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` are clean.
- Branch `leapp-rs-f1`, made from `master`. Never stage `.nvmrc` (the owner's own change).
- One new commit per task and per fix. No amend, no squash. Conventional commit messages that end
  with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Tests never touch the real home folder, the real keychain or real AWS: `tempfile::TempDir` home,
  `MemoryStore`, `fake-aws`.
- Secrets (access keys, secret keys, session tokens, MFA codes, the user's backed-up section) never
  go in `workspace.json`, logs, `Debug` output, error text or the element tree. Request types that
  hold a secret have a hand-written `Debug` that prints `<redacted>`.
- `leapp-core` must not depend on Rune.
- Element keys are the test contract. Keep the F0 keys (`row-<id>`, `status-<id>`, `cell-*-<id>`,
  `btn-create`, `btn-create-submit`, `btn-create-cancel`, `fld-*`, `err-*`, `sel-region*`,
  `dlg-iam-user`, `dlg-overwrite`, `btn-overwrite-yes`, `btn-overwrite-no`, `toast-error`,
  `toast-error-close`, `empty-state`).
- Spec values, verbatim:
  - `GetSessionToken` `DurationSeconds = 36000`.
  - Refresh tick 10 s. A session is refreshed when `now - start_time > 1200 s` (strictly greater).
  - MFA dialog title: `MFA code for <session name>`. Cancel error text:
    `Missing Multi Factor Authentication code`.
  - Keychain keys: `iam-user-<id>-access-key-id`, `iam-user-<id>-secret-access-key`,
    `iam-user-<id>-session-token`, `aws-profile-backup-<profile>`.
  - Row menu labels: `Start session` / `Stop session`, `Edit session`, `Copy account number`,
    `Delete session`. Delete confirm text: `Delete session <name>?`.
  - Empty state: `No sessions yet` and a button `Add session`.
- Rulings that bind this plan (record each in the ledger at setup):
  - **F1-R1** (confirmed by the owner on 2026-11-24) The token is reused only while `session_token_expiration - now > 1200 s`
    (`TOKEN_REUSE_MARGIN`, equal to the refresh age). Reason: written credentials then stay valid
    until the next refresh. Spec 11.1 says `> now`; cost if wrong: one extra `GetSessionToken` per
    10 h.
  - **F1-R2** (confirmed by the owner on 2026-11-24) The spec's F1 refresh acceptance test ("21 min → new credentials") conflicts with the
    token cache of 11.1 step 1 (Leapp also reuses the token). Tests check: after 21 min a tick
    rewrites the section and moves `start_time` with no new `GetSessionToken`; after the token
    expires, a tick gets and writes new credentials.
  - **F1-R3** (confirmed by the owner on 2026-11-24) Edit of an `Active` session that changes the profile, region, keys or MFA device:
    stop, save, start again (the same rule as F2 change region/profile). A name-only edit does not
    restart. Edit or delete of a `Pending` session returns `Error::ProfilePending` and changes
    nothing (like R6).
  - **F1-R4** A change of the keys or of the MFA device deletes the cached token and clears
    `session_token_expiration`. A change of the keys also clears `account`.
  - **F1-R5** `account` comes from `GetCallerIdentity` with the temporary credentials, after a
    start that called `GetSessionToken`, only while `account` is `None`. A failure of that call is
    dropped (logging is F8, R8); the start still succeeds.
  - **F1-R6** Delete saves the workspace first, then deletes the keychain keys. A failed key delete
    is dropped (an orphan key under a deleted session's UUID is harmless; R8).
  - **F1-R7** Core has no timer. `refresh_tick()` does one pass; the app calls it every 10 s.
  - **F1-R8** A stop whose profile had `had_original` but whose keychain backup is missing removes
    the app's section, completes the stop, and then returns `Error::BackupMissing(profile)` so the
    UI shows it.
  - **F1-R9** Chained-session cascade on delete (spec 10.3) and pins are not built: chained roles
    are F3 and pins are F6. Those plans extend `delete_session`.

## Tasks at a glance

Each task has one implementer subagent and one reviewer subagent (the reviewer also runs
rust-review). After Task 9, one reviewer checks the whole branch.

| Task | Crate | What it builds |
|---|---|---|
| 1 | core | F0 review fixes: no backup or STS call after a quit; keychain reads outside the workspace lock; an error for a missing backup |
| 2 | core | MFA on start (cancel → `Missing Multi Factor Authentication code`); a `Field` enum for input errors |
| 3 | core | Token cache: a new token when less than 20 min is left; the account number from `GetCallerIdentity` after the first start |
| 4 | core | Edit (an active session stops, saves, starts again; new keys drop the cached token) and delete (the user's own profile comes back) |
| 5 | core | `refresh_tick`: rewrites sessions older than 20 min; a failure stops the session |
| 6 | app | MFA dialog (6 digits only, Enter submits) in one question queue; the overwrite buttons look disabled for the first 0.5 s |
| 7 | app | One form for create and edit: MFA device field, eye toggle on the secret, the core call off the UI thread, errors clear when you type, the dialog fits the smallest window |
| 8 | app | Row menu (⋯ and right-click): Start/Stop, Edit, Copy account number, Delete with a confirm dialog; "Add session" button in the empty state |
| 9 | app | Refresh loop every 10 s, failures in the toast; `CLAUDE.md` and the carry-forward list updated |

Not in F1: deletion of chained sessions with their parent (F3) and pins (F6).

After Task 9, the owner checks with real AWS: an IAM User with MFA starts, edit, delete and Copy
account number work, and a session stays active after 21 min.

## Review Focus

1. A user types an MFA code with a space or only 5 digits → the Submit button stays disabled. A code
   with spaces around it is trimmed. (Task 6)
2. A user edits or deletes a session while it is `Pending` → a clear "is starting" error, and no
   change to the workspace, the file or the keychain. (Task 4)
3. A user deletes an `Active` session whose profile had the user's own section → that section comes
   back byte for byte and the backup key is gone. (Task 4)
4. A session that the user stopped just before a refresh tick → the tick makes no STS call and does
   not write the section back. (Task 5)
5. A refresh needs a new token, the session has an MFA device, and the user cancels the MFA dialog →
   the session goes `Inactive`, its section is removed, and the error names the session and says
   `Missing Multi Factor Authentication code`. (Task 5)

---

### Task 0 (controller, no subagent): Branch

- [ ] `git checkout master && git pull --ff-only && git checkout -b leapp-rs-f1`.
- [ ] Record the rulings F1-R1 to F1-R9 in the ledger.

### Task 1: Core hardening from the F0 reviews

**Files:**
- Modify: `crates/leapp-core/src/lib.rs`, `src/sessions/mod.rs`, `src/sessions/iam_user.rs`,
  `src/error.rs`
- Modify: `crates/leapp-core/tests/common/mod.rs`
- Test: `crates/leapp-core/tests/hardening.rs` (new)

**Interfaces:**
- Produces: `Leapp` keeps its state in one `Mutex<State>`, `struct State { ws: Workspace, closing:
  bool }` (private). The `closing: AtomicBool` field goes away.
- Produces: `Error::BackupMissing(String)`, message: `the saved copy of profile "<P>" is missing
  from the keychain item "aws-profile-backup-<P>" (service leapp-rs). Leapp RS removed its own keys
  from the profile, but could not put your keys back.`
- Produces (tests/common): `FlakyStore` — wraps `MemoryStore`; `fail_set_after(n: usize)` makes the
  `(n+1)`-th `set` from now fail with `Error::Secret`; `block_get(key_prefix: &str) -> Gate`
  makes `get` of keys with that prefix wait until `Gate::open()`. `Harness::with_store(Arc<FlakyStore>)`.

- [ ] **Step 1: Write the failing tests** in `tests/hardening.rs`:
  - `a_start_that_waits_on_aws_at_shutdown_leaves_no_backup`: user section `[default]` exists,
    prompter says yes, `fake.delay_next("GetSessionToken", 300 ms)`; spawn `start`; settle until
    `fake.count("GetSessionToken") == 1`; call `shutdown` while the start waits on AWS. Assert
    the start returns `Error::Closing`, the credentials file equals the user's text byte for byte,
    and `secrets.snapshot()` has no `aws-profile-backup-default`.
  - `a_start_after_shutdown_makes_no_sts_call`: `shutdown`, then `start` → `Error::Closing` and
    `fake.count("GetSessionToken") == 0`.
  - `a_stop_does_not_hold_the_lock_while_the_keychain_reads`: user section, start (yes), then
    `store.block_get("aws-profile-backup-")`; run `stop` on a thread; from the test thread,
    `leapp.workspace()` must return within 200 ms while the gate is closed; open the gate; the stop
    ends `Ok` and the user section is back.
  - `a_failed_second_secret_write_leaves_no_session_and_no_keys`: `fail_set_after(1)`;
    `create_iam_user` → `Err(Error::Secret(_))`; the workspace has no session and the store has no
    `iam-user-` key.
  - `a_stop_with_a_missing_backup_says_so`: user section, start (yes), delete the
    `aws-profile-backup-default` key by hand, `stop` → `Err(Error::BackupMissing(p))` with
    `p == "default"`; afterwards the session is `Inactive`, `[default]` is not in the file, and
    `managed_profiles` is empty.
  - `an_aws_error_with_an_empty_message_shows_the_full_error`: `fake.fail_next("GetSessionToken",
    403, "AccessDenied", "")`; `start` → `Error::Aws(m)` with `!m.is_empty()`.
- [ ] **Step 2:** `cargo test -p leapp-core --test hardening` → the tests fail (compile errors for
  the new helpers and variant count as failing).
- [ ] **Step 3: Implement.**
  - `closing` becomes a `bool` in `State`; every check reads it under the same lock as the
    workspace.
  - `activate` checks `closing` under the lock before it writes the backup, and
    `iam_user_credentials` checks it before the STS call.
  - `stop` and `clean_all` read the backup keys before they take the lock for the write, then check
    under the lock that the managed entry did not change (a changed entry: `stop` starts again from
    the top one time; `clean_all` skips that profile, and the next `startup` does it). The backup
    delete (`Released::finish`) runs after the guard is dropped.
  - `release_profile` with `had_original` and no backup removes the section and records the
    missing backup; `stop` and `clean_all` return `Error::BackupMissing` after the save.
  - `aws_message` treats `Some("")` like `None`.
- [ ] **Step 4:** `cargo test --workspace` → all pass (the F0 tests too).
- [ ] **Step 5: Commit** `fix(leapp-core): close the F0 review gaps in shutdown, locks and backups`.

### Task 2: MFA on start, and a field enum for input errors

**Files:**
- Modify: `crates/leapp-core/src/config.rs`, `src/model.rs`, `src/error.rs`, `src/lib.rs`,
  `src/sessions/iam_user.rs`
- Modify: `crates/leapp-core/tests/common/mod.rs`, `crates/leapp-app/src/bridge.rs` (only so the
  workspace compiles: `UiPrompter::ask_mfa_code` returns `None` until Task 6),
  `crates/leapp-app/src/views/create_dialog.rs` (field mapping uses the enum),
  every `NewIamUser { .. }` literal (add `mfa_device: None`)
- Test: `crates/leapp-core/tests/mfa.rs` (new)

**Interfaces:**
- Produces: `Prompter::ask_mfa_code(&self, session_name: &str) -> Option<String>` (async, no
  default body). `None` means the user cancelled.
- Produces: `NewIamUser.mfa_device: Option<String>`. Core trims it; empty after trim means `None`.
  It is stored in `SessionKind::IamUser { mfa_device, .. }`.
- Produces: `pub enum Field { Name, AccessKeyId, SecretAccessKey, Profile, Region, MfaDevice }`,
  exported from `leapp_core`, `Display` as `name`, `access_key_id`, `secret_access_key`, `profile`,
  `region`, `mfa_device`. `Error::InvalidInput { field: Field, message: String }`.
- Produces (tests/common): `ScriptedPrompter::mfa_answers(&self, answers: Vec<Option<&str>>)` (a
  queue; empty queue answers `None`) and `ScriptedPrompter::mfa_asked(&self) -> Vec<String>`.

- [ ] **Step 1: Write the failing tests** in `tests/mfa.rs`:
  - `a_session_with_an_mfa_device_sends_the_code`: create with
    `mfa_device: Some(" arn:aws:iam::123456789012:mfa/dev ")`, `mfa_answers([Some("123456")])`,
    start → `Ok`; the last `GetSessionToken` request has `SerialNumber ==
    "arn:aws:iam::123456789012:mfa/dev"`, `TokenCode == "123456"`, `DurationSeconds == "36000"`;
    `mfa_asked() == ["<session name>"]`.
  - `a_cancelled_mfa_code_fails_the_start`: `mfa_answers([None])`, start →
    `Err(e)` with `e.to_string() == "Missing Multi Factor Authentication code"`; the session is
    `Inactive`; `fake.count("GetSessionToken") == 0`; the credentials file is unchanged.
  - `a_session_without_an_mfa_device_never_asks`: start → `mfa_asked()` is empty and the request
    has no `SerialNumber` and no `TokenCode`.
  - `an_empty_mfa_device_is_stored_as_none`: create with `Some("   ")` → the session's
    `mfa_device` is `None`, and `workspace.json` has `"mfa_device":null`.
  - `an_input_error_names_its_field`: create with an empty name →
    `Error::InvalidInput { field: Field::Name, .. }`; with region `"xx-nowhere-1"` →
    `field: Field::Region`.
- [ ] **Step 2:** `cargo test -p leapp-core --test mfa` → fails.
- [ ] **Step 3: Implement.** Add `Error::MfaCancelled` with the exact text
  `Missing Multi Factor Authentication code`. In `iam_user_credentials`, when a new token is needed
  and `mfa_device` is set, call `ask_mfa_code(&session.name)` (no lock held), trim the code, and
  pass `serial_number` and `token_code`.
- [ ] **Step 4:** `cargo test --workspace` → all pass.
- [ ] **Step 5: Commit** `feat(leapp-core): ask for the MFA code when an IAM User needs a token`.

### Task 3: Token cache rules, and the account number

**Files:**
- Modify: `crates/fake-aws/src/lib.rs`, `crates/fake-aws/tests/sts.rs`
- Modify: `crates/leapp-core/src/sessions/iam_user.rs`, `src/sessions/mod.rs`, `src/lib.rs`
- Modify: `crates/leapp-core/tests/common/mod.rs`
- Test: `crates/leapp-core/tests/token_cache.rs` (new)

**Interfaces:**
- Produces: `pub const TOKEN_REUSE_MARGIN: chrono::Duration` = 1200 s (F1-R1), exported.
- Produces (fake-aws): `GetCallerIdentity` answers `Account` = `FakeAws::account()` (default
  `"123456789012"`, settable with `set_account(&str)`), `Arn =
  arn:aws:iam::<account>:user/fake`, `UserId = AIDAFAKE`. It records the request like the others.
- Produces (tests/common): `ManualClock` (starts at `Utc::now()`, `advance(chrono::Duration)`,
  `set(DateTime<Utc>)`), and `Harness::with_manual_clock() -> (Harness, Arc<ManualClock>)`.

- [ ] **Step 1: Write the failing tests.**
  - fake-aws `tests/sts.rs`: `get_caller_identity_returns_the_account`: an SDK call with any keys
    returns account `"123456789012"`; after `set_account("210987654321")` it returns that.
  - `token_cache.rs`:
    - `a_second_start_reuses_the_token`: start, stop, start → `fake.count("GetSessionToken") == 1`,
      and the file has `fake-token-1` both times.
    - `an_expired_token_is_replaced`: start, stop, `clock.advance(36001 s)`, start → count is 2,
      the file has `fake-token-2`.
    - `a_token_close_to_expiry_is_replaced`: start, stop, `clock.advance(36000 s - 600 s)`, start →
      count is 2.
    - `a_token_with_time_left_is_reused`: start, stop, `clock.advance(36000 s - 1800 s)`, start →
      count is 1.
    - `the_first_start_stores_the_account`: start → `kind.account == Some("123456789012")` and
      `fake.count("GetCallerIdentity") == 1`; stop, start → still 1 call.
    - `a_failed_caller_identity_does_not_fail_the_start`:
      `fake.fail_next("GetCallerIdentity", 500, "InternalFailure", "boom")`, start → `Ok`, status
      `Active`, `account == None`.
- [ ] **Step 2:** `cargo test -p fake-aws && cargo test -p leapp-core --test token_cache` → fails.
- [ ] **Step 3: Implement.** Reuse the token only if
  `expiration - clock.now() > TOKEN_REUSE_MARGIN`. After a start that called `GetSessionToken` and
  saved `Active`, if `account` is `None`, call `GetCallerIdentity` with the temporary credentials
  (session region, same timeout and retry config) and save the account (no lock during the call).
- [ ] **Step 4:** `cargo test --workspace` → all pass.
- [ ] **Step 5: Commit** `feat(leapp-core): reuse the IAM User token until near expiry and keep the account`.

### Task 4: Edit and delete

**Files:**
- Modify: `crates/leapp-core/src/model.rs`, `src/lib.rs` (or a new `src/sessions/edit.rs`)
- Test: `crates/leapp-core/tests/edit_delete.rs` (new)

**Interfaces:**
- Produces: `pub struct IamUserUpdate { pub name: String, pub profile_name: String, pub region:
  String, pub mfa_device: Option<String>, pub access_key_id: Option<String>, pub secret_access_key:
  Option<String> }`. `None` (or empty after trim) key fields mean "keep the old value". `Debug`
  redacts both keys.
- Produces: `pub async fn update_iam_user(&self, id: SessionId, req: IamUserUpdate) -> Result<()>`
  — the same validation and `Field` errors as `create_iam_user`; F1-R3 and F1-R4.
- Produces: `pub async fn delete_session(&self, id: SessionId) -> Result<()>` — spec 10.3 without
  the cascade (F1-R9); F1-R6.

- [ ] **Step 1: Write the failing tests** in `tests/edit_delete.rs`:
  - `edit_with_new_keys_drops_the_token_and_uses_the_new_keys`: start, stop, update with
    `access_key_id: Some("AKID-NEW")`, `secret_access_key: Some("SECRET-NEW")` → the store has no
    `iam-user-<id>-session-token`, `session_token_expiration == None`, `account == None`; start →
    the last `GetSessionToken` request has `access_key_id == Some("AKID-NEW")`.
  - `edit_with_empty_keys_keeps_the_old_keys`: update with `Some("")` and `None` → the store still
    has `AKID-<name>` and `SECRET-<name>`; the token key is still there.
  - `edit_of_an_active_session_moves_its_section`: start on `p-a`; update profile to `p-b` → the
    file has no `[p-a]`, has `[p-b]`, the session is `Active`.
  - `edit_of_only_the_name_does_not_restart`: start; update the name → `fake.count` unchanged and
    `start_time` unchanged.
  - `edit_of_a_pending_session_is_refused`: `delay_next("GetSessionToken", 300 ms)`, spawn start,
    wait until the status is `Pending`, update → `Err(Error::ProfilePending(_))`; after the start
    ends, the name is the old name.
  - `edit_validates_like_create`: an empty name → `Field::Name`; nothing changed.
  - `delete_removes_the_session_keys_and_section`: start, delete → no session in the workspace, no
    `iam-user-<id>-` key in the store, no `[default]` in the file, `managed_profiles` empty.
  - `delete_puts_the_users_section_back`: user `[default]` text, start (yes), delete → the file
    equals the user's text byte for byte and there is no `aws-profile-backup-default`.
  - `delete_of_a_pending_session_is_refused`: as the edit case → `Error::ProfilePending`; the session
    and its keys stay.
  - `delete_of_an_unknown_session_is_not_found`: → `Error::NotFound(id)`.
- [ ] **Step 2:** `cargo test -p leapp-core --test edit_delete` → fails.
- [ ] **Step 3: Implement** with the existing `stop` and `start`; no new file-writing code.
- [ ] **Step 4:** `cargo test --workspace` → all pass.
- [ ] **Step 5: Commit** `feat(leapp-core): edit and delete IAM User sessions`.

### Task 5: Refresh

**Files:**
- Create: `crates/leapp-core/src/refresh.rs`
- Modify: `crates/leapp-core/src/lib.rs`, `src/sessions/mod.rs`
- Test: `crates/leapp-core/tests/refresh.rs` (new)

**Interfaces:**
- Produces: `pub const REFRESH_TICK: std::time::Duration` = 10 s and
  `pub const SESSION_DURATION: chrono::Duration` = 1200 s, exported.
- Produces: `pub async fn refresh_tick(&self) -> RefreshReport`;
  `pub struct RefreshReport { pub refreshed: Vec<SessionId>, pub failures: Vec<RefreshFailure> }`;
  `pub struct RefreshFailure { pub session_id: SessionId, pub session_name: String, pub error: Error }`.
  `RefreshReport::is_empty(&self) -> bool`.

- [ ] **Step 1: Write the failing tests** in `tests/refresh.rs` (manual clock):
  - `a_young_session_is_not_refreshed`: start, `advance(1200 s)`, tick → report empty, the file is
    unchanged.
  - `an_old_session_is_written_again_with_the_cached_token`: start, `advance(21 min)`, tick →
    `refreshed == [id]`, `start_time` equals the clock's now, `fake.count("GetSessionToken") == 1`
    (F1-R2).
  - `an_old_session_with_an_expired_token_gets_new_credentials`: start, `advance(36001 s)`, tick →
    the file has `fake-token-2`; the session is `Active`.
  - `a_refresh_error_stops_the_session`: start, `advance(36001 s)`,
    `fail_next("GetSessionToken", 403, "ExpiredToken", "The security token expired")`, tick →
    `failures` has one entry with that session's name and an error whose text contains
    `The security token expired`; status `Inactive`; no `[default]` in the file; `managed_profiles`
    empty.
  - `a_refresh_error_puts_the_users_section_back`: as above with a user `[default]` section (yes) →
    the file equals the user's text byte for byte.
  - `a_stopped_session_is_not_refreshed` (Review Focus 4): start, stop, `advance(21 min)`, tick →
    report empty, no new STS request, no `[default]` in the file.
  - `a_cancelled_mfa_on_refresh_stops_the_session` (Review Focus 5): session with an MFA device,
    `mfa_answers([Some("123456"), None])`, start, `advance(36001 s)`, tick → one failure whose
    error text is `Missing Multi Factor Authentication code`; status `Inactive`; section removed.
  - `a_tick_after_shutdown_does_nothing`: start, `advance(21 min)`, shutdown, tick → report empty.
- [ ] **Step 2:** `cargo test -p leapp-core --test refresh` → fails.
- [ ] **Step 3: Implement.** For each session that is `Active` with `now - start_time >
  SESSION_DURATION`, under the lock: if still `Active` and not closing, set `Pending`; then run the
  existing `activate` path (the profile is already managed by this session, so it asks no
  overwrite question). On an error other than `Error::Closing`, release the profile the same way
  `stop` does and set `Inactive`. Sessions are refreshed one after another.
- [ ] **Step 4:** `cargo test --workspace` → all pass.
- [ ] **Step 5: Commit** `feat(leapp-core): refresh old active sessions`.

### Task 6: App — question queue and MFA dialog

**Files:**
- Create: `crates/leapp-app/src/views/mfa_dialog.rs`
- Modify: `crates/leapp-app/src/lib.rs`, `src/bridge.rs`, `src/views/mod.rs`,
  `src/views/overwrite_dialog.rs`
- Modify: `crates/leapp-app/tests/common/mod.rs`
- Test: `crates/leapp-app/tests/mfa.rs` (new), `tests/create_and_start.rs` (count rename)

**Interfaces:**
- Consumes: `Prompter::ask_mfa_code` (Task 2).
- Produces: `Msg::AskMfa { session: String, reply: oneshot::Sender<Option<String>> }`;
  `UiPrompter::ask_mfa_code` sends it and awaits it (a dropped reply means `None`).
- Produces: the `Ui` holds one queue of questions (`enum Question { Overwrite {..}, Mfa {..} }`),
  shown one at a time with the same arming delay (`CONFIRM_ARM_SECONDS`).
  `pending_confirm()` stays; `pending_mfa() -> Option<&str>` (the session name);
  `pending_confirm_count()` is renamed `pending_question_count()`.
- Produces: keys `dlg-mfa` (body), `fld-mfa-code`, `btn-mfa-submit`, `btn-mfa-cancel`. Dialog
  title `MFA code for <session name>`.
- Produces (tests/common): `AppHarness::with_setup` gives the app a session with an MFA device
  through `NewIamUser.mfa_device`.

- [ ] **Step 1: Write the failing tests** in `tests/mfa.rs`:
  - `the_mfa_dialog_sends_the_code`: session with an MFA device; double-press the row; settle until
    `pending_mfa().is_some()`; the dialog title text is `MFA code for <name>`; `wait_for_arming`;
    type `123456`; tap `btn-mfa-submit` → settles `Active`; the fake's last request has
    `TokenCode == "123456"`.
  - `enter_submits_the_code`: as above, type then `press(Key::Enter)` → `Active`.
  - `a_short_or_spaced_code_cannot_be_submitted` (Review Focus 1): type `12345` → tapping
    `btn-mfa-submit` leaves the dialog open and makes no request; type `1 2345` likewise; clear and
    type ` 123456 ` (spaces around) → submit works and the request has `TokenCode == "123456"`.
  - `cancel_shows_the_mfa_error`: tap `btn-mfa-cancel` → `toast-error` text is
    `Missing Multi Factor Authentication code`; status `inactive`.
  - `the_overwrite_buttons_look_disabled_until_armed`: an overwrite question shows → before
    `wait_for_arming` both `btn-overwrite-yes` and `-no` report disabled (rune-kit `Button` disabled
    state, read from the element tree); after it, both are enabled.
  - `closing_the_window_with_the_mfa_dialog_open_quits`: open the MFA dialog, call
    `leapp_app::on_close` → returns within 2 s; the start ends with an error; the file has no
    `[default]`.
- [ ] **Step 2:** `cargo test -p leapp-app --test mfa` → fails.
- [ ] **Step 3: Implement.** The code field is a `TextInput` with auto focus; Submit is enabled
  only when the trimmed text is exactly 6 ASCII digits. The `TextEdit` of the code is secure (it
  wipes its buffers). Disable both overwrite buttons until armed.
- [ ] **Step 4:** `cargo test --workspace` → all pass.
- [ ] **Step 5: Commit** `feat(leapp-app): ask for the MFA code in a dialog`.

### Task 7: App — one form for create and edit

**Files:**
- Modify: `crates/leapp-app/src/views/create_dialog.rs` (rename to `session_form.rs` is allowed),
  `src/lib.rs`, `src/bridge.rs`
- Test: `crates/leapp-app/tests/create_and_start.rs`, `tests/edit.rs` (new)

**Interfaces:**
- Consumes: `update_iam_user`, `IamUserUpdate`, `Field`, `NewIamUser.mfa_device` (Tasks 2, 4).
- Produces: `CreateForm` gets a mode, `Create` or `Edit(SessionId)`; `pub(crate) fn
  open_edit(s: &mut Ui, id: SessionId)` fills it from the workspace (secret fields empty).
- Produces: `Msg::FormDone(Result<(), leapp_core::Error>)`; submit runs the core call with
  `Ui::run`-style spawning (not on the render thread) and sets `saving = true`; Submit is disabled
  while saving.
- Produces: keys `fld-mfa-device`, `err-mfa-device`, `btn-secret-eye` (eye toggle on the secret
  field). Edit mode: title `Edit IAM User`, submit label `Save`, same keys as create.
  Secret field placeholders in edit mode: `Leave empty to keep the current key`.

- [ ] **Step 1: Write the failing tests.**
  - `create_and_start.rs`:
    - `each_input_error_shows_under_its_field`: for each of access key (empty), secret key (empty)
      and a form error (make the secret store fail with the harness `FlakyStore`) → the text shows
      under `err-access-key`, `err-secret-key`, `err-form`.
    - `typing_in_a_field_clears_its_error`: submit empty → `err-name` shows; type a letter in
      `fld-name` → `err-name` is gone, `err-access-key` still shows.
    - `the_eye_shows_the_secret`: type a secret; the field is secure (`has_secure_focus` after a tap
      on it); tap `btn-secret-eye` → not secure; tap again → secure.
    - `an_mfa_device_is_saved`: fill `fld-mfa-device` → the created session has that `mfa_device`.
    - `the_dialog_fits_the_smallest_window`: `set_viewport(spec().min_size)`, open the dialog → the
      rects of `btn-create-submit` and `btn-create-cancel` lie inside the viewport.
  - `edit.rs`:
    - `edit_keeps_secret_fields_empty_and_keeps_old_keys`: open edit for a session → `fld-name` has
      the name, `fld-secret-key` and `fld-access-key` are empty; change the name; Save → the
      workspace has the new name and the store still has the old keys.
    - `edit_with_new_keys_uses_them`: type new keys, Save, start → the fake's last request has the
      new access key ID.
    - `edit_errors_show_under_the_field`: clear the name, Save → `err-name` shows, dialog stays
      open.
- [ ] **Step 2:** `cargo test -p leapp-app` → the new tests fail.
- [ ] **Step 3: Implement.** Map `Error::InvalidInput { field, .. }` with a `match` on `Field` (no
  strings). Clearing on edit: a field's `on_edit` clears its own error.
- [ ] **Step 4:** `cargo test --workspace` → all pass. Render
  `cargo run -p leapp-app -- --snap target/shots/create-dialog.png --scene create-dialog` and look
  at it.
- [ ] **Step 5: Commit** `feat(leapp-app): edit IAM User sessions in the session form`.

### Task 8: App — row menu, delete confirm, empty state button

**Files:**
- Create: `crates/leapp-app/src/views/row_menu.rs`, `src/views/delete_dialog.rs`
- Modify: `crates/leapp-app/src/views/main_window.rs`, `src/lib.rs`
- Modify: `crates/leapp-app/tests/common/mod.rs`
- Test: `crates/leapp-app/tests/row_menu.rs` (new), `tests/main_window.rs`

**Interfaces:**
- Consumes: `delete_session`, `open_edit` (Tasks 4, 7); `kind.account` (Task 3).
- Produces: a `⋯` `IconButton` keyed `row-menu-<id>` at the end of each row (before the status),
  and `on_secondary_press` on the row; both open a rune-kit menu keyed `menu-<id>` with the items,
  in this order: `Start session` or `Stop session`, `Edit session`, `Copy account number`
  (disabled while `account` is `None`), `Delete session` (danger).
- Produces: delete confirm keys `dlg-delete`, `btn-delete-yes` (label `Delete`), `btn-delete-no`
  (label `Cancel`); text `Delete session <name>?`.
- Produces: `btn-empty-add-session` (label `Add session`) in the empty state; it opens the create
  form.
- Produces: the `Ui` keeps `starting: HashSet<SessionId>`; a workspace read keeps those sessions
  `Pending` until their start ends (carry-forward F1 item).
- Produces (tests/common): `secondary_tap(key)`, `tap_menu_item(id: SessionId, label: &str)` (finds
  the element under `menu-<id>-item*` whose text is `label`), and a `rune::clipboard::memory()`
  clipboard set with `rt.use_clipboard`, exposed as `AppHarness::clipboard_text() -> Option<String>`.
- The MIN_WINDOW_WIDTH formula includes the new column if the `⋯` button adds width; the
  narrow-window test keeps passing.

- [ ] **Step 1: Write the failing tests** in `tests/row_menu.rs`:
  - `the_menu_starts_and_stops`: tap `row-menu-<id>` → tap `Start session` → `active`; open again,
    the item reads `Stop session` → tap → `inactive`.
  - `right_click_opens_the_same_menu`: `secondary_tap("row-<id>")` → `menu-<id>-menu` exists.
  - `copy_account_number_copies_it`: start, stop; open the menu → `Copy account number` → 
    `clipboard_text() == Some("123456789012")`.
  - `copy_account_number_is_disabled_before_the_first_start`: tapping it copies nothing.
  - `delete_asks_then_deletes`: menu → `Delete session` → `dlg-delete` text is
    `Delete session <name>?`; `btn-delete-no` → the session stays; again, `btn-delete-yes` → no
    `row-<id>`, `empty-state` shows, the store has no `iam-user-<id>-` key.
  - `edit_opens_the_form_for_that_session`: menu → `Edit session` → the form title is
    `Edit IAM User` and `fld-name` has the name.
  - `main_window.rs`: `the_empty_state_has_an_add_session_button`: tap `btn-empty-add-session` →
    the create form is open.
  - `main_window.rs`: `a_starting_session_stays_pending_through_other_updates`:
    `delay_next("GetSessionToken", 300 ms)`, double-press the row, send `Msg::Refreshed` at once →
    `status-<id>` still reads `pending` on the next frame; it reads `active` at the end.
- [ ] **Step 2:** `cargo test -p leapp-app` → the new tests fail.
- [ ] **Step 3: Implement.** Copy uses `Tx::copy`. Delete runs off the render thread.
- [ ] **Step 4:** `cargo test --workspace` → all pass. Render the `one-session` scene and look at
  the row with its `⋯`.
- [ ] **Step 5: Commit** `feat(leapp-app): row menu with edit, delete and copy account number`.

### Task 9: App — refresh loop, docs

**Files:**
- Create: `crates/leapp-app/src/refresh.rs`
- Modify: `crates/leapp-app/src/lib.rs`, `src/main.rs`, `leapp-rs/CLAUDE.md`,
  `docs/superpowers/plans/2026-11-24-leapp-rs-f0-carry-forward.md`
- Modify: `crates/leapp-app/tests/common/mod.rs` (manual clock in `Config`)
- Test: `crates/leapp-app/tests/refresh.rs` (new)

**Interfaces:**
- Consumes: `refresh_tick`, `RefreshReport`, `REFRESH_TICK` (Task 5).
- Produces: `pub fn spawn_refresh_loop(leapp: Arc<Leapp>, rt: &tokio::runtime::Handle, bridge:
  Bridge, every: std::time::Duration) -> tokio::task::JoinHandle<()>`. Each tick: for each failure
  send `Msg::Failed("Refresh of <session name> failed: <error>")`; if the report is not empty send
  `Msg::Refreshed`. `main.rs` calls it with `REFRESH_TICK` after `startup`.

- [ ] **Step 1: Write the failing tests** in `tests/refresh.rs` (manual clock, `every` = 10 ms):
  - `the_loop_writes_new_credentials_after_the_token_expires`: start through the UI, `advance(36001
    s)`, spawn the loop → settle until the credentials file has `fake-token-2`; the row reads
    `active`.
  - `a_refresh_failure_shows_in_the_toast`: start, `advance(36001 s)`, `fail_next(...,
    "The security token expired")`, spawn → `toast-error` text is
    `Refresh of <name> failed: The security token expired`; the row reads `inactive`.
- [ ] **Step 2:** `cargo test -p leapp-app --test refresh` → fails.
- [ ] **Step 3: Implement.** Update `CLAUDE.md` (new keys: `row-menu-<id>`, `menu-<id>`, the MFA
  and delete dialog keys; how tests pick menu items by label; the refresh loop and the manual
  clock). Update the carry-forward doc: mark each F1 item done or say where it moved.
- [ ] **Step 4:** `cargo test --workspace`, clippy, fmt → clean.
- [ ] **Step 5: Commit** `feat(leapp-app): refresh active sessions in the background`.

### After Task 9

- Final whole-branch review (most capable model, with rust-review), one fix wave.
- Owner check with real AWS (spec 15.4): an IAM User with MFA starts;
  `aws sts get-caller-identity --profile <p>` works; edit, delete and Copy account number work; a
  session stays active after 21 min.
