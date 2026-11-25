# Leapp RS F2 + F3 (Named profiles, regions, IAM Role Chained) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Named profiles that the user can create, rename and delete; a default region in
settings; "Change region…" and "Change named profile…" on a session; and IAM Role Chained sessions
that assume a role with the credentials of a parent session, with edit and a delete cascade.

**Architecture:** All AWS, file and keychain work stays in `leapp-core` behind the `Leapp` facade.
One core helper (`change.rs`) does "reserve, stop the active sessions, save a change, start them
again" for change region, change profile, rename profile and delete profile. Chained credentials
come from a recursive `session_credentials` that gets the parent's credentials (without a start of
the parent and without a write of its profile) and then calls `AssumeRole`. The app adds a settings
dialog, two small row-menu dialogs, a session-type chooser and a chained-role form that shares the
profile and region fields with the IAM User form.

**Tech Stack:** Rust 2024, tokio, `aws-sdk-sts` (Query protocol), jiff, serde, Rune + rune-kit at
rev `3d4e2dfcecb71890f7f4d6167e82fe6db9dbb695`, `fake-aws` (axum).

**Spec:** `docs/superpowers/specs/2026-11-23-leapp-rs-design.md`. Read sections 6, 9.2, 9.3, 9.4,
10, 11.1, 11.2, 12, 13, 14.1, 14.3, 14.5, 14.7, 14.8, 15 and 16 (F2, F3). Also read the F2 section,
the "Small items" and the rulings table of `docs/superpowers/plans/2026-11-24-leapp-rs-f0-carry-forward.md`,
and `leapp-rs/CLAUDE.md`. Leapp references:
`packages/core/src/services/named-profiles-service.ts` (rename, delete, validation),
`packages/core/src/services/regions-service.ts`,
`packages/core/src/services/session/aws/aws-iam-role-chained-service.ts`.

## Global Constraints

- Run all commands in `leapp-rs/`. Done means `cargo test --workspace` passes,
  `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` are clean.
- Branch `leapp-rs-f2-f3`, made from `master`. Never stage `.nvmrc` (the owner's own change).
- One new commit per task and per fix. No amend, no squash. Conventional commit messages that end
  with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Tests never touch the real home folder, the real keychain or real AWS: `tempfile::TempDir` home,
  `MemoryStore`, `fake-aws`. The app is never run un-headless by an agent.
- Secrets (secret access keys, session tokens, MFA codes, the user's backed-up section) never go in
  `workspace.json`, logs, `Debug` output, error text or the element tree. Request types that hold a
  secret have a hand-written `Debug` that prints `<redacted>`.
- The access key ID is an identifier, not a secret (owner ruling 2026-11-24): it may show in the UI
  and the element tree. It never goes in `workspace.json`, logs, `Debug` output or error text, and
  it is stored only in the `SecretStore`.
- A role ARN, a role session name and an external ID are not secrets (AWS documents the external
  ID as not secret). They are saved in `workspace.json`.
- `leapp-core` must not depend on Rune.
- Element keys are the test contract. Keep all F0 and F1 keys (see `leapp-rs/CLAUDE.md`).
- `workspace.json` stays `version: 1`. A file written by F1 (no `settings`, no chained sessions)
  must load unchanged; new fields get `#[serde(default)]`.
- Spec values, verbatim:
  - `AssumeRole` `DurationSeconds = 3600` (`ASSUME_ROLE_DURATION`). Default role session name
    `assumed-from-leapp`.
  - Default region `us-east-1`.
  - Profile name errors (Leapp `named-profiles-service.ts:117`): `Empty profile name`,
    `"default" is not a valid profile name`, `Profile already exists`.
  - Row menu labels: `Start session` / `Stop session`, `Change region…`, `Change named profile…`,
    `Copy account number`, `Copy role ARN`, `Edit session`, `Delete session` (in this order;
    spec 14.3 without the F6/F7 items). `…` is U+2026.
  - Delete confirm: `Delete session <name>?`, and when it has chained sessions below it, a second
    sentence: `N chained sessions that use it will also be deleted.` (for N = 1:
    `1 chained session that uses it will also be deleted.`)
  - Type chooser labels: `AWS IAM User`, `AWS IAM Role Chained`.
  - Chained form labels: `Session alias`, `Assumer session`, `Role ARN`, `Role session name`,
    `External ID`, `Named profile`, `Region`. Provider chip text `IAM Role Chained`, purple.
  - Settings tabs: `General`, `Profiles`. General has `Default region`.
- Rulings that bind this plan (record each in the ledger at setup; the owner reviews them with the
  plan):
  - **F23-R1** `change_profile` takes a profile *name*, not an ID: the dialog is "select or type a
    new name" (spec 14.7). A new name creates the profile, as `create_iam_user` does.
  - **F23-R2** The `default` profile can be neither renamed nor deleted (spec 12: "always exists").
    Both return `InvalidInput { field: Profile, message: "the default profile cannot be renamed or deleted" }`.
  - **F23-R3** A change that keeps the value (same region, same profile, rename to the same name)
    does nothing: no stop, no save, no restart.
  - **F23-R4** Change region, change profile, rename profile and delete profile on an `Active`
    session: stop, save, start again (spec 9.3 step 8, F1-R3, Leapp). If the restart fails, the
    change stays saved, the session is `Inactive`, and the start's error is returned (same as
    F1 edit). A `Pending` session, or one that an edit or change holds, returns
    `Error::ProfilePending` and nothing changes.
  - **F23-R5** Delete of a profile asks a confirm in the app (armed for `CONFIRM_ARM_SECONDS`):
    `Delete profile <name>? N sessions that use it will move to the default profile.` (the second
    sentence only when N > 0; for N = 1: `1 session that uses it will move to the default profile.`).
  - **F23-R6** The delete cascade is transitive: deleting a session deletes every chained session
    below it at any depth (spec 10.3 says "whose parent is this session"; a grandchild would
    otherwise point to a deleted parent). All of them stop first if `Active`. If any of them is
    `Pending` or held by an edit, the delete returns `Error::ProfilePending` and changes nothing.
  - **F23-R7** The parent's credentials for a chained start use the parent's own rules: an IAM
    User parent reuses its cached token (F1-R1) and asks for its MFA code only when it needs a new
    token. The MFA dialog names the **parent** (`MFA code for <parent name>`), because the device
    belongs to the parent. A new parent token is saved on the parent
    (`session_token_expiration`, keychain token), guarded by the parent's key generation, so the
    next chained start and the parent's own start reuse it. The parent's status and profile
    section do not change.
  - **F23-R8** Chained sessions have no token cache (Leapp has none): each start and each refresh
    calls `AssumeRole`. `session_token_expiration` is saved for display.
  - **F23-R9** A chained start or refresh is refused (start: `Error::ProfilePending`; refresh:
    skipped) while the session or any session above it is held by an edit.
  - **F23-R10** A role ARN is valid when it matches `arn:<partition>:iam::<12 digits>:role/<path and name>`
    where partition is `aws`, `aws-cn` or `aws-us-gov`. The account number is the 12 digits (the
    5th `:` field); for the `aws` partition this equals spec 11.2 "chars 13..25".
  - **F23-R11** Optional fields follow the AWS `AssumeRole` limits: role session name 2–64 chars of
    `[A-Za-z0-9+=,.@_-]`; external ID 2–1224 chars of `[A-Za-z0-9+=,.@:/_-]`. Empty after trim
    means "not set" (role session name then sends `assumed-from-leapp`).
  - **F23-R12** The type chooser shows only `AWS IAM User` and `AWS IAM Role Chained`. `AWS Single
    Sign-On` comes with F4.
  - **F23-R13** The settings dialog has only the `General` (Default region) and `Profiles` tabs.
    F8 adds the other settings and tabs. The Role column of the table is F6 (spec F6 scope).
  - **F23-R14** Edit of an `Active` chained session restarts it when anything but the name changes
    (F1-R3 for chained). `update_iam_user` on a chained session, and `update_iam_role_chained` on
    an IAM User session, return `Error::WrongSessionType(id)`.
  - **F23-R15** A chain with a loop in `workspace.json` (only possible by a hand edit) makes the
    start fail with `Error::ChainLoop(<session name>)`; it never recurses without end.

## Tasks at a glance

Each task has one implementer subagent and one reviewer subagent (the reviewer also runs
rust-review). After Task 11, one reviewer checks the whole branch.

| Task | Crate | What it builds |
|---|---|---|
| 1 | core | Settings with the default region; profile name validation; `create_profile`; the change helper; `change_region` and `change_profile` |
| 2 | core | `rename_profile` and `delete_profile` (sessions move to `default`, active ones restart) |
| 3 | fake-aws, core | `AssumeRole` in fake-aws; the chained session model, create, and start through a parent (one and two levels) |
| 4 | core | Parent MFA and parent token reuse; refresh of chained sessions; edit guard on ancestors; loop guard |
| 5 | core | Edit of chained sessions (loop rejected) and the transitive delete cascade |
| 6 | app | Shared profile and region fields: the Named profile field becomes a select with "create"; new sessions preselect the default region |
| 7 | app | Settings dialog: General (default region) and Profiles (add, rename, delete with confirm) |
| 8 | app | Row menu: `Change region…` and `Change named profile…` dialogs; new menu order |
| 9 | app | Type chooser and the IAM Role Chained create form; purple provider chip |
| 10 | app | Chained edit, `Copy role ARN`, account number of chained sessions, delete confirm with the chained count |
| 11 | docs | `CLAUDE.md` keys and gotchas; carry-forward list; headless scenes for the new dialogs |

After Task 11, the owner checks with real AWS (spec 15.4): an IAM Role Chained session with an IAM
User parent (with MFA) starts, `aws sts get-caller-identity --profile <p>` shows the assumed role,
change region and change profile keep it active, and a profile rename moves its section.

## Review Focus

1. A user starts a session while a change of its region or profile is between its stop and its
   save → the start is refused with the "is starting" error and the session ends `Active` with the
   new value. (Task 1)
2. A user renames a profile to a name that already has a section they wrote by hand in
   `~/.aws/credentials`, while a session on it is `Active` → the restart asks the overwrite
   question; "no" leaves their section byte for byte, the profile renamed, the session `Inactive`,
   and the error returned. (Task 2)
3. A user deletes a profile whose session is `Active` while another session on `default` is also
   `Active` → the moved session ends `Active` on `default`, and the other `default` session is
   `Inactive` (one active session per profile). (Task 2)
4. A chained session whose parent has wrong keys → the start fails with the AWS error, makes no
   `AssumeRole` call, writes nothing, and the chained session is `Inactive`. (Task 3)
5. A `workspace.json` with a loop of chained sessions (hand edit) → the start returns
   `Error::ChainLoop` at once and the app does not hang or overflow its stack. (Task 4)

---

### Task 0 (controller, no subagent): Branch

- [ ] `git checkout master && git pull --ff-only && git checkout -b leapp-rs-f2-f3`.
- [ ] Record the rulings F23-R1 to F23-R15 in the ledger.

### Task 1: Settings, profile validation, change region and change profile (core)

**Files:**
- Modify: `crates/leapp-core/src/model.rs`, `src/lib.rs`, `src/error.rs`
- Create: `crates/leapp-core/src/profiles.rs` (validation, `create_profile`),
  `crates/leapp-core/src/sessions/change.rs` (the change helper, `change_region`, `change_profile`)
- Test: `crates/leapp-core/tests/profiles.rs` (new), `crates/leapp-core/tests/workspace_compat.rs`

**Interfaces:**
- Produces (model): `pub struct Settings { pub default_region: String }` with `Default`
  (`us-east-1`), `Workspace.settings: Settings` with `#[serde(default)]`. `Serialize`,
  `Deserialize`, `Clone`, `Debug`, `PartialEq`.
- Produces (facade):
  - `pub fn update_settings(&self, settings: Settings) -> Result<()>`: validates
    `default_region` against `regions::REGIONS` (`InvalidInput { field: Region, .. }`), saves.
  - `pub fn create_profile(&self, name: &str) -> Result<Uuid>`: trims; validates with
    `validate_new_profile_name`; saves.
  - `pub(crate) fn validate_new_profile_name(ws: &Workspace, name: &str, except: Option<Uuid>) -> Result<String>`:
    the three Leapp messages (Global Constraints), plus the F0 rule "must not contain [, ], or
    line breaks"; `except` skips that profile in the duplicate check (for rename). Returns the
    trimmed name. All errors are `InvalidInput { field: Field::Profile, .. }`.
  - `pub async fn change_region(&self, id: SessionId, region: &str) -> Result<()>`
  - `pub async fn change_profile(&self, id: SessionId, profile_name: &str) -> Result<()>`: the
    name is trimmed and checked with the F0 character rule; an unknown name creates the profile
    (F23-R1); `default` is allowed as a target.
  - `pub(crate) async fn change_sessions(&self, ids: &[SessionId], change: impl FnOnce(&mut Workspace) -> Result<()>) -> Result<()>`
    in `sessions/change.rs`. Steps: (1) under the lock, refuse if any id is `Pending`, held in
    `editing`, or on a profile that is `Pending` (`pending_error()`/`editing_error()`); reserve
    all ids in `editing` with one drop guard (generalize `Editing` to a set of ids, or add
    `EditingMany`); note which are `Active`. (2) stop each `Active` one (keep the first
    `BackupMissing` and go on, as F1-R8). (3) under the lock, check that each id still exists and
    is `Inactive` (else `changed_meanwhile()`), apply `change` to a clone, save, swap. (4) drop
    the reservation, then start again the ones that were `Active`; return the first start error,
    else the `BackupMissing`, else `Ok`.
- Produces (errors): `Error::ProfileNotFound(Uuid)` (`"named profile {0} not found"`).

- [ ] **Step 1: Write the failing tests** in `tests/profiles.rs` (core `Harness`, `fake-aws`):
  - `change_region_of_an_active_session_rewrites_its_section`: start a session in `us-east-1`;
    `change_region(id, "eu-west-1")`; the section's `region = eu-west-1`; status `Active`;
    `workspace().session(id).region == "eu-west-1"`; `fake.count("GetSessionToken") == 1`
    (token reused, F1-R1).
  - `change_profile_of_an_active_session_moves_its_section`: active on `default`;
    `change_profile(id, "dev")`; `[default]` gone, `[dev]` has the 4 keys; a profile `dev`
    exists; status `Active`.
  - `a_change_that_keeps_the_value_does_nothing` (F23-R3): `change_region(id, <same>)` and
    `change_profile(id, <same>)` on an active session → `start_time` unchanged and the
    credentials file text byte-identical.
  - `a_change_of_a_pending_session_changes_nothing`: hold the start on `fake.delay_next` so the
    session is `Pending`; `change_region` → `Err(ProfilePending)`; region unchanged on disk.
  - `a_start_during_a_change_is_refused` (Review Focus 1): user section `[default]` exists
    (so stop reads the backup); start with "yes"; `store.block_get("aws-profile-backup-")`; run
    `change_region` on a thread; `gate.wait_blocked()`; `start(id)` →
    `Err(ProfilePending)`; open the gate; the change ends `Ok`, region new, status `Active`.
  - `profile_names_are_checked`: `create_profile("  ")` → message `Empty profile name`;
    `create_profile("default")` → `"default" is not a valid profile name`; create `dev` twice →
    `Profile already exists`; `create_profile("a]b")` → field `Profile`; `create_profile(" ci ")`
    → a profile named `ci`.
  - `the_default_region_setting_is_saved`: `update_settings(Settings { default_region: "eu-west-1".into() })`;
    reopen the harness; `workspace().settings.default_region == "eu-west-1"`; `"xx-nowhere-1"` →
    `InvalidInput { field: Region, .. }`.
  - In `workspace_compat.rs`, extend `an_f0_workspace_file_still_loads`: the fixture has no
    `settings` → `settings.default_region == "us-east-1"`.
- [ ] **Step 2: Run** `cargo test -p leapp-core --test profiles` → FAIL (functions not defined).
- [ ] **Step 3: Implement** the interfaces above. `change_region` validates the region first
  (`InvalidInput { field: Region }`) and returns `Ok` at once for F23-R3; same for
  `change_profile`.
- [ ] **Step 4: Run** `cargo test -p leapp-core` → PASS; then the three workspace commands.
- [ ] **Step 5: Commit** `feat(leapp-core): settings, profile names, change region and profile`.

### Task 2: Rename and delete a named profile (core)

**Files:**
- Modify: `crates/leapp-core/src/profiles.rs`
- Test: `crates/leapp-core/tests/profiles.rs`

**Interfaces:**
- Consumes: `change_sessions`, `validate_new_profile_name` (Task 1).
- Produces:
  - `pub async fn rename_profile(&self, profile_id: Uuid, name: &str) -> Result<()>`: F23-R2,
    F23-R3; validation with `except = Some(profile_id)`; uses `change_sessions` with the ids of
    the sessions on the profile, and the change renames the profile.
  - `pub async fn delete_profile(&self, profile_id: Uuid) -> Result<()>`: F23-R2; uses
    `change_sessions` with the ids on the profile; the change sets their `profile_id` to the
    `default` profile's ID and removes the profile. A profile with no sessions is only removed.
  - `impl Workspace { pub fn sessions_on(&self, profile_id: Uuid) -> usize }` (the app's confirm
    text needs it).

- [ ] **Step 1: Write the failing tests:**
  - `rename_of_a_profile_with_an_active_session_moves_its_section`: active on `dev`;
    `rename_profile(dev_id, "stage")` → `[dev]` gone, `[stage]` written, status `Active`, the
    profile ID unchanged.
  - `rename_to_a_user_written_section_asks_first` (Review Focus 2): active on `dev`; the file also
    has a user section `[stage]`; prompter answers "no"; `rename_profile(dev_id, "stage")` →
    `Err(ProfileOverwriteDeclined("stage"))`; the profile is named `stage`; status `Inactive`;
    the `[stage]` text byte-identical; `[dev]` gone.
  - `delete_profile_moves_its_sessions_to_default`: two sessions on `dev` (one `Active`);
    `delete_profile(dev_id)` → no profile `dev`; both sessions on the `default` profile ID; the
    active one `Active` with a `[default]` section; `[dev]` gone.
  - `a_moved_session_takes_over_default` (Review Focus 3): A active on `default`, B active on
    `dev`; `delete_profile(dev_id)` → B `Active`, A `Inactive`, `[default]` holds B's keys
    (`fake` request log: B's start was the last `GetSessionToken`, or compare the token values).
  - `the_default_profile_cannot_be_renamed_or_deleted`: both calls → the F23-R2 message; the
    workspace text unchanged.
  - `rename_checks_the_name`: rename `dev` to `default`, to an existing `ci`, to `"  "` → the
    three Leapp messages; rename `dev` to `" dev "` → `Ok`, nothing changes (F23-R3).
  - `rename_or_delete_of_an_unknown_profile_fails`: random ID → `Err(ProfileNotFound(id))`.
- [ ] **Step 2: Run** `cargo test -p leapp-core --test profiles` → the new tests FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p leapp-core` → PASS; workspace commands clean.
- [ ] **Step 5: Commit** `feat(leapp-core): rename and delete named profiles`.

### Task 3: AssumeRole and IAM Role Chained sessions (fake-aws, core)

**Files:**
- Modify: `crates/fake-aws/src/lib.rs`, `crates/fake-aws/tests/sts.rs`
- Modify: `crates/leapp-core/src/model.rs`, `src/lib.rs`, `src/error.rs`,
  `src/sessions/mod.rs` (`write_active` dispatch), `src/sessions/iam_user.rs` (share
  `sts_client`, `aws_message`, the expiration conversion)
- Create: `crates/leapp-core/src/sessions/chained.rs`
- Test: `crates/leapp-core/tests/chained.rs` (new)

**Interfaces:**
- Produces (fake-aws):
  - `Recorded.session_token: Option<String>` from the `x-amz-security-token` header.
  - `AssumeRole` answer: `AccessKeyId` `ASIACHAIN{n:04}`, `SecretAccessKey` `fake-chained-secret-{n}`,
    `SessionToken` `fake-chained-token-{n}`, `Expiration` = now + `DurationSeconds` (default
    3600), plus `AssumedRoleUser` with `Arn` = `<RoleArn with :role/ replaced by :assumed-role/>/<RoleSessionName>`.
    `fail_next("AssumeRole", ..)` and `delay_next` work as for the other actions.
- Produces (model):
  - `SessionKind::IamRoleChained { role_arn: String, parent_session_id: SessionId, role_session_name: Option<String>, external_id: Option<String> }`
    (serde tag `iam_role_chained`, spec 12).
  - `impl Session { pub fn account(&self) -> Option<String> }`: IAM User → `account`; chained →
    from the role ARN (F23-R10).
  - `pub struct NewIamRoleChained { pub name, pub parent_session_id: SessionId, pub role_arn, pub role_session_name: Option<String>, pub external_id: Option<String>, pub profile_name, pub region }`
    (derived `Debug`: no secrets).
- Produces (facade): `pub fn create_iam_role_chained(&self, req: NewIamRoleChained) -> Result<SessionId>`;
  `pub const ASSUME_ROLE_DURATION: i32 = 3600`; `pub const DEFAULT_ROLE_SESSION_NAME: &str = "assumed-from-leapp"`.
- Produces (crate): `pub(crate) async fn session_credentials(&self, session: &Session, seen: &mut Vec<SessionId>) -> Result<Issued>`
  in `chained.rs`: IAM User → `iam_user_credentials`; chained → the parent's
  `session_credentials` (boxed recursion: `Box::pin`), then `AssumeRole` with the parent's
  temporary keys, in the chained session's region (10.5). `write_active` calls it instead of the
  `match` on the kind.
- Produces (errors): `Field::{ParentSession, RoleArn, RoleSessionName, ExternalId}` (Display
  `parent_session`, `role_arn`, `role_session_name`, `external_id`);
  `Error::WrongSessionType(Uuid)` (`"session {0} has another type"`);
  `Error::ChainLoop(String)` (`"the parent sessions of \"{0}\" make a loop"`).
- Validation (create): name required; parent exists (`InvalidInput { field: ParentSession, message: "session not found" }`);
  role ARN F23-R10 (`message: "must look like arn:aws:iam::123456789012:role/name"`);
  F23-R11 for the optional fields; profile and region as for IAM User. A new session can not be in
  a loop, so create needs no loop check.

- [ ] **Step 1: Write the failing tests.** fake-aws: in `tests/sts.rs`,
  `assume_role_answers_and_records_the_token`: a signed `AssumeRole` call with a session token →
  recorded `params["RoleArn"]`, `params["DurationSeconds"] == "3600"`, `session_token ==
  Some(<token sent>)`; answer parses with `aws-sdk-sts`. Core, `tests/chained.rs`:
  - `a_chained_start_assumes_the_role_with_the_parents_token`: IAM User parent P on `default`
    (inactive), chained C on `ci` with `role_arn = "arn:aws:iam::123456789012:role/deploy"`,
    `external_id = Some("ext-1")`; `start(C)` → the request log has `GetSessionToken` then
    `AssumeRole`; the `AssumeRole` record has `access_key_id == Some("ASIAFAKE0001")`,
    `session_token == Some("fake-token-1")`, `RoleSessionName == "assumed-from-leapp"`,
    `DurationSeconds == "3600"`, `ExternalId == "ext-1"`; `[ci]` has `fake-chained-token-1`;
    no `[default]` section; P stays `Inactive`; C `Active` with
    `session_token_expiration` about now + 3600 s.
  - `a_set_role_session_name_is_sent`: `role_session_name = Some("me")` → `RoleSessionName == "me"`;
    no `ExternalId` param when `external_id` is `None`.
  - `a_two_level_chain_works`: P → C1 → C2; `start(C2)` → two `AssumeRole` records; the second
    one is signed with C1's temporary key (`ASIACHAIN0001`); `[<C2 profile>]` has
    `fake-chained-token-2`; C1 stays `Inactive` and has no section.
  - `a_parent_with_wrong_keys_stops_the_chain` (Review Focus 4): `fake.fail_next("GetSessionToken", 403, "InvalidClientTokenId", "The security token included in the request is invalid.")`;
    `start(C)` → `Err(Aws(msg))` with that message; `fake.count("AssumeRole") == 0`; credentials
    file unchanged; C `Inactive`.
  - `an_assume_role_error_is_shown`: `fail_next("AssumeRole", 403, "AccessDenied", "not allowed")`
    → `Err(Aws(m))`, `m` contains `not allowed`; C `Inactive`; file unchanged.
  - `chained_input_is_checked`: one assert per rule — empty name (`Name`), unknown parent
    (`ParentSession`), `role_arn = "deploy"` (`RoleArn`), `role_session_name = Some("x")`
    (`RoleSessionName`), `external_id = Some("a b")` (`ExternalId`); and nothing saved for each.
  - `a_chained_session_round_trips_in_the_workspace_file`: create C; reopen; the session equals
    the one before, and the JSON has `"type": "iam_role_chained"` (parse with `serde_json`).
  - `the_account_comes_from_the_role_arn`: `session.account() == Some("123456789012")`; for
    `arn:aws-cn:iam::210987654321:role/x` → `Some("210987654321")`.
- [ ] **Step 2: Run** `cargo test -p fake-aws && cargo test -p leapp-core --test chained` → FAIL.
- [ ] **Step 3: Implement.** Make the existing irrefutable `let SessionKind::IamUser { .. } = ..`
  patterns in `edit.rs` and `iam_user.rs` return `Error::WrongSessionType(id)` for a chained
  session (F23-R14); `account_missing` stays IAM-User-only. The app must still build, with the
  smallest change: `provider_label` (`views/main_window.rs:250`) returns `IAM Role Chained`;
  `FieldErrors::slot` (`views/create_dialog.rs:89`) puts the four new fields in the `form` slot;
  `open_edit` (`create_dialog.rs:271`) and the account in `row_menu.rs:64` do nothing for a
  chained session (Tasks 9 and 10 build them).
- [ ] **Step 4: Run** `cargo test --workspace` → PASS; clippy and fmt clean.
- [ ] **Step 5: Commit** `feat(leapp-core): IAM Role Chained sessions assume a role through their parent`.

### Task 4: Parent MFA, parent token reuse, chained refresh, guards (core)

**Files:**
- Modify: `crates/leapp-core/src/sessions/chained.rs`, `src/sessions/mod.rs`,
  `src/sessions/iam_user.rs`
- Test: `crates/leapp-core/tests/chained.rs`

**Interfaces:**
- Consumes: `session_credentials` (Task 3), `key_generation` (F1), `store_account` pattern.
- Produces: `pub(crate) fn save_parent_token(&self, parent: SessionId, expiration: Timestamp, key_generation: u64)`
  (saves `session_token_expiration` on the parent only when its key generation is unchanged;
  a failed save is dropped, as F1-R5). Start and `refresh_session` check F23-R9 in the critical
  section that sets `Pending` (walk the parent chain with a visited set; F23-R15 gives
  `ChainLoop`).

- [ ] **Step 1: Write the failing tests:**
  - `the_parents_mfa_is_asked_when_its_token_is_old`: parent P with an MFA device; prompter answers
    `"123456"`; `start(C)` → `mfa_asked() == ["P"]` (F23-R7); `GetSessionToken` has
    `TokenCode=123456`; C `Active`.
  - `the_parents_token_is_reused` (F23-R7): after the start above, `stop(C)`, `start(C)` → one
    `GetSessionToken` in total, two `AssumeRole`, `mfa_asked().len() == 1`; then `start(P)` →
    still one `GetSessionToken`.
  - `a_parent_token_near_expiry_is_renewed`: advance the clock so less than 1200 s are left
    (F1-R1) → the next `start(C)` asks MFA again and makes a second `GetSessionToken`.
  - `cancel_of_the_parents_mfa_fails_the_chained_start`: prompter cancels → `Err(MfaCancelled)`;
    `AssumeRole` count 0; C `Inactive`.
  - `an_old_chained_session_is_refreshed`: C active; advance 1201 s; `refresh_tick()` → a second
    `AssumeRole`, `[ci]` has `fake-chained-token-2`, `start_time` moved, no second
    `GetSessionToken` (F23-R8 + F1-R1).
  - `a_chained_start_waits_for_an_edit_of_its_parent` (F23-R9): `block_set("iam-user-<P>-")`;
    edit P's keys on a thread; `wait_blocked`; `start(C)` → `Err(ProfilePending)`; open the gate;
    after the edit, `start(C)` signs `GetSessionToken` with the new key ID.
  - `a_refresh_skips_a_chained_session_whose_parent_is_edited`: same gate; C active and due;
    `refresh_tick()` → empty report, `AssumeRole` count unchanged.
  - `a_loop_in_the_file_fails_fast` (Review Focus 5, F23-R15): write a `workspace.json` with
    C1.parent = C2 and C2.parent = C1 (both chained, valid otherwise); open; `start(C1)` →
    `Err(ChainLoop("C1"))` within the test's normal time; no AWS request.
- [ ] **Step 2: Run** `cargo test -p leapp-core --test chained` → the new tests FAIL (the reuse,
  refresh-skip and loop tests at least).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` → PASS; clippy and fmt clean.
- [ ] **Step 5: Commit** `feat(leapp-core): chained sessions reuse the parent token and refresh`.

### Task 5: Edit of chained sessions and the delete cascade (core)

**Files:**
- Modify: `crates/leapp-core/src/sessions/edit.rs`, `src/sessions/chained.rs`, `src/model.rs`
- Test: `crates/leapp-core/tests/chained.rs`, `crates/leapp-core/tests/edit_delete.rs`

**Interfaces:**
- Produces:
  - `pub struct IamRoleChainedUpdate` with the fields of `NewIamRoleChained` (derived `Debug`).
  - `pub async fn update_iam_role_chained(&self, id: SessionId, req: IamRoleChainedUpdate) -> Result<()>`:
    the validation of create plus: parent is not `id` (`message: "must not be the session itself"`)
    and not below `id` (`message: "would make a loop of chained sessions"`); F23-R14 restart
    rules; the `editing` reservation as `update_iam_user`.
  - `impl Workspace { pub fn descendants(&self, id: SessionId) -> Vec<SessionId> }`: all chained
    sessions below `id`, any depth, each once.
  - `delete_session` follows F23-R6.

- [ ] **Step 1: Write the failing tests:**
  - `edit_of_a_chained_session_saves_its_fields`: change role ARN, role session name, external
    ID, profile, region on an `Inactive` C → all saved; no AWS call.
  - `edit_of_an_active_chained_session_restarts_it`: C active; change the role ARN → a second
    `AssumeRole` with the new `RoleArn`; status `Active`. A name-only edit → no new `AssumeRole`.
  - `a_loop_is_rejected_on_edit`: P → C1 → C2; set C1's parent to C2 →
    `InvalidInput { field: ParentSession, message: "would make a loop of chained sessions" }`;
    C1's parent to C1 → `"must not be the session itself"`; workspace text unchanged.
  - `the_wrong_update_function_is_refused`: `update_iam_user(C, ..)` and
    `update_iam_role_chained(P, ..)` → `Err(WrongSessionType(_))`.
  - `delete_of_a_parent_deletes_its_chain` (spec F3 acceptance + F23-R6): P → C1 → C2, C2 active;
    `delete_session(P)` → no P, C1, C2 in the workspace; C2's section gone; P's keychain keys gone;
    the credentials file has no section of the chain.
  - `delete_of_a_parent_with_a_pending_child_changes_nothing`: C1 `Pending` (delay the
    `AssumeRole`); `delete_session(P)` → `Err(ProfilePending)`; all three sessions and P's keys
    still there.
  - `descendants_lists_each_session_once`: a parent with two children and one grandchild → 3
    ids.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` → PASS; clippy and fmt clean.
- [ ] **Step 5: Commit** `feat(leapp-core): edit chained sessions and delete them with their parent`.

### Task 6: Shared profile and region fields (app)

**Files:**
- Create: `crates/leapp-app/src/views/fields.rs` (`profile_field`, `region_field`, `field` moved
  from `create_dialog.rs`)
- Modify: `crates/leapp-app/src/views/create_dialog.rs`, `src/views/mod.rs`, `src/lib.rs`
- Test: `crates/leapp-app/tests/profiles_field.rs` (new); update `tests/create_and_start.rs` and
  `tests/edit.rs` where they type into `fld-profile`

**Interfaces:**
- Produces: `pub(crate) struct ProfileChoice { pub name: String, pub combo: ComboboxState }` and
  `pub(crate) fn profile_field(key: &str, choice: &ProfileChoice, profiles: &[NamedProfile], on: impl Fn(&mut Ui) -> &mut ProfileChoice + Clone + 'static, k: &Kit, layer: &mut Vec<El>) -> El`
  (or an equivalent shape that keeps one function for every dialog). Element keys: a wrapper
  `sel-profile`, trigger `sel-profile-value-trigger`, options `sel-profile-option-<name>`, and,
  when the search text (trimmed) is not empty and is not an existing name, one more option keyed
  `sel-profile-option-__new__` with the label `Create profile "<text>"`. Choosing it sets the name
  to the trimmed search text. `region_field` is the F0/F1 region `Combobox` (keys `sel-region*`
  unchanged).
- The IAM User form (create) starts with `region = workspace().settings.default_region`
  (spec F2 acceptance; carry-forward F2) and `profile = "default"`.

- [ ] **Step 1: Write the failing tests:**
  - `the_profile_field_lists_the_profiles`: two profiles `default`, `dev`; open the IAM User
    form; tap `sel-profile-value-trigger` → `has("sel-profile-option-default")` and
    `has("sel-profile-option-dev")`.
  - `a_new_profile_name_can_be_typed`: type `stage` after opening the field → tap
    `sel-profile-option-__new__`; fill the rest; save → the new session's profile name is
    `stage` (core `workspace()`).
  - `an_existing_name_offers_no_create_option`: type `dev` → no `sel-profile-option-__new__`.
  - `the_form_preselects_the_default_region`: `update_settings(eu-west-1)` before the open →
    `create_form().region() == "eu-west-1"` and the trigger shows `eu-west-1`.
- [ ] **Step 2: Run** `cargo test -p leapp-app --test profiles_field` → FAIL.
- [ ] **Step 3: Implement.** Replace the `fld-profile` text field; keep `err-profile`.
- [ ] **Step 4: Run** `cargo test --workspace` → PASS (update the F0/F1 tests that typed into
  `fld-profile` to use the select; the assertions stay the same).
- [ ] **Step 5: Commit** `feat(leapp-app): the named profile field is a select with create`.

### Task 7: Settings dialog (app)

**Files:**
- Create: `crates/leapp-app/src/views/settings_dialog.rs`
- Modify: `crates/leapp-app/src/views/main_window.rs` (toolbar button), `src/lib.rs`
- Test: `crates/leapp-app/tests/settings.rs` (new)

**Interfaces:**
- Consumes: `update_settings`, `create_profile`, `rename_profile`, `delete_profile`,
  `Workspace::sessions_on` (Tasks 1–2); `region_field` (Task 6); the question arming of F1.
- Produces (keys): `btn-settings` (toolbar, gear icon, label `Settings`), `dlg-settings`,
  `tab-general`, `tab-profiles` (if rune-kit `SegmentedControl` gives no key per segment, use two
  buttons in a row with these keys), `sel-default-region` (region `Combobox`, options
  `sel-default-region-option-<region>`), `profile-row-<id>`, `profile-name-<id>`,
  `btn-profile-rename-<id>`, `fld-profile-rename-<id>`, `btn-profile-rename-save-<id>`,
  `err-profile-rename-<id>`, `btn-profile-delete-<id>` (no rename or delete button for
  `default`), `fld-new-profile`, `btn-profile-add`, `err-new-profile`, `dlg-delete-profile`,
  `btn-delete-profile-yes`, `btn-delete-profile-no`, `btn-settings-close`.
- Rename and delete run off the render thread (they can restart sessions); their errors show in
  the toast; a validation error shows under the field.

- [ ] **Step 1: Write the failing tests:**
  - `the_default_region_is_set_in_settings`: tap `btn-settings`, `tab-general`, choose
    `eu-west-1` in `sel-default-region` → core `settings.default_region == "eu-west-1"`; then
    `btn-create` → the IAM User form shows `eu-west-1`.
  - `a_profile_is_added`: `tab-profiles`, type `dev` in `fld-new-profile`, tap `btn-profile-add`
    → a `profile-row-<id>` with text `dev`; core has the profile. Type `default` → `err-new-profile`
    text `"default" is not a valid profile name`.
  - `a_profile_is_renamed`: rename `dev` to `stage` → `profile-name-<id>` text `stage`; a session
    on it shows `stage` in `cell-profile-<sid>`.
  - `delete_asks_and_moves_the_sessions`: a session on `dev`; tap `btn-profile-delete-<id>` →
    `dlg-delete-profile` with text
    `Delete profile dev? 1 session that uses it will move to the default profile.`;
    `btn-delete-profile-yes` is disabled until `wait_for_arming()`; yes → no `profile-row-<id>`;
    `cell-profile-<sid>` text `default`.
  - `the_default_profile_has_no_rename_or_delete`: no `btn-profile-rename-<default id>`, no
    `btn-profile-delete-<default id>`.
- [ ] **Step 2: Run** `cargo test -p leapp-app --test settings` → FAIL.
- [ ] **Step 3: Implement.** Copy the look of Leapp `options-dialog` (spec 14.8).
- [ ] **Step 4: Run** `cargo test --workspace` → PASS; clippy and fmt clean.
- [ ] **Step 5: Commit** `feat(leapp-app): settings dialog with default region and profiles`.

### Task 8: Change region and change named profile (app)

**Files:**
- Create: `crates/leapp-app/src/views/change_dialogs.rs`
- Modify: `crates/leapp-app/src/views/row_menu.rs`, `src/lib.rs`
- Test: `crates/leapp-app/tests/change.rs` (new), `tests/row_menu.rs`

**Interfaces:**
- Consumes: `change_region`, `change_profile` (Task 1); `region_field`, `profile_field` (Task 6).
- Produces: row menu items in the Global Constraints order (chained-only `Copy role ARN` comes in
  Task 10). Keys: `dlg-change-region`, `sel-change-region` (Combobox, options
  `sel-change-region-option-<region>`), `btn-change-region-save`, `btn-change-region-cancel`,
  `dlg-change-profile`, `sel-change-profile` (profile field), `btn-change-profile-save`,
  `btn-change-profile-cancel`. The dialog opens with the session's current value. The core call
  runs off the render thread with the `starting`/`save_result` pattern of F1 (a panic ends it);
  its error shows in the toast.

- [ ] **Step 1: Write the failing tests:**
  - `change_region_from_the_menu`: active session; `tap_menu_item(id, "Change region…")`;
    choose `eu-west-1`; save → `cell-region-<id>` text `EU-WEST-1`;
    the credentials file has `region = eu-west-1`; `status-<id>` label `active`.
  - `change_profile_from_the_menu`: type `ci`, choose `sel-change-profile-option-__new__`, save →
    `cell-profile-<id>` text `ci`; file has `[ci]`, no `[default]`.
  - `cancel_changes_nothing`: open, choose another region, cancel → region unchanged.
  - `the_menu_has_the_new_order`: the menu item labels in order equal `Start session`,
    `Change region…`, `Change named profile…`, `Copy account number`, `Edit session`,
    `Delete session`.
- [ ] **Step 2: Run** `cargo test -p leapp-app --test change` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` → PASS.
- [ ] **Step 5: Commit** `feat(leapp-app): change region and named profile from the row menu`.

### Task 9: Type chooser and the IAM Role Chained form (app)

**Files:**
- Create: `crates/leapp-app/src/views/type_chooser.rs`, `src/views/chained_dialog.rs`
- Modify: `crates/leapp-app/src/views/main_window.rs` (`btn-create` and `btn-empty-add-session`
  open the chooser; provider chip), `src/lib.rs`
- Test: `crates/leapp-app/tests/chained.rs` (new); update tests that tapped `btn-create` and
  expected `dlg-iam-user` directly

**Interfaces:**
- Consumes: `create_iam_role_chained` (Task 3); `profile_field`, `region_field` (Task 6).
- Produces (keys): `dlg-choose-type`, `btn-type-iam-user`, `btn-type-iam-role-chained`;
  `dlg-iam-role-chained`, `fld-chained-name`, `sel-parent` (a `Select` of all sessions by name;
  in edit mode without the session itself and its descendants; options
  `sel-parent-option-<session id>`), `fld-role-arn`, `fld-role-session-name` (placeholder
  `assumed-from-leapp`), `fld-external-id`, `sel-profile`, `sel-region`, `err-chained-name`,
  `err-parent`, `err-role-arn`, `err-role-session-name`, `err-external-id`, `err-profile`,
  `err-region`, `err-chained-form`, `btn-chained-submit` (disabled until name, parent and role
  ARN are filled), `btn-chained-cancel`. Provider chip text `IAM Role Chained`, purple
  (`#8b5cf6` or the Leapp SCSS value).
- `pub struct ChainedForm` with `Debug`; an `InvalidInput` error goes under its field by `Field`.

- [ ] **Step 1: Write the failing tests:**
  - `add_session_asks_for_the_type`: tap `btn-create` → `dlg-choose-type` with the two labels;
    `btn-type-iam-user` → `dlg-iam-user`.
  - `a_chained_session_is_created_and_started`: an IAM User P exists; chooser →
    `btn-type-iam-role-chained`; fill name `deploy`, parent P, role ARN, profile `ci` (create);
    submit → a row with `IAM Role Chained` and `ci`; tap `status-<id>` → `active`; the file has
    `[ci]` with `fake-chained-token-1`.
  - `chained_errors_show_under_the_field`: role ARN `deploy` → `err-role-arn` text has
    `must look like arn:aws:iam::123456789012:role/name`; role session name `x` →
    `err-role-session-name` shows.
  - `the_submit_waits_for_the_required_fields`: `btn-chained-submit` disabled until name, parent
    and role ARN are set.
- [ ] **Step 2: Run** `cargo test -p leapp-app --test chained` → FAIL.
- [ ] **Step 3: Implement.** Labels from Global Constraints; layout from
  `docs/images/screens/newuxui/aws-iam-role-chained.png`.
- [ ] **Step 4: Run** `cargo test --workspace` → PASS.
- [ ] **Step 5: Commit** `feat(leapp-app): create IAM Role Chained sessions`.

### Task 10: Chained edit, Copy role ARN, account number, delete count (app)

**Files:**
- Modify: `crates/leapp-app/src/views/chained_dialog.rs`, `src/views/row_menu.rs`,
  `src/views/delete_dialog.rs`, `src/lib.rs`
- Test: `crates/leapp-app/tests/chained.rs`, `tests/row_menu.rs`

**Interfaces:**
- Consumes: `update_iam_role_chained`, `Workspace::descendants`, `Session::account` (Tasks 3, 5).
- Produces: `Edit session` on a chained row opens `dlg-iam-role-chained` in edit mode (title
  `Edit IAM Role Chained`, submit label `Save`); `Copy role ARN` (chained rows only, after `Copy
  account number`); `Copy account number` uses `Session::account()` and is disabled when it is
  `None`; the delete confirm adds the second sentence of the Global Constraints when
  `descendants(id).len() > 0`.

- [ ] **Step 1: Write the failing tests:**
  - `a_chained_session_is_edited`: change role ARN and save → core has the new ARN; an `Active`
    one is `active` again after the restart.
  - `the_parent_select_leaves_out_the_session_and_its_chain`: P → C1 → C2; edit C1 → options for
    P only (no `sel-parent-option-<C1>`, no `sel-parent-option-<C2>`).
  - `copy_role_arn_and_account_of_a_chained_session`: `Copy role ARN` → clipboard = the ARN;
    `Copy account number` → `123456789012`. An IAM User row has no `Copy role ARN`.
  - `delete_of_a_parent_names_its_chain`: P with two chained below → `dlg-delete` text
    `Delete session P? 2 chained sessions that use it will also be deleted.`; yes → three rows gone.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace` → PASS.
- [ ] **Step 5: Commit** `feat(leapp-app): edit chained sessions, copy role ARN, delete the chain`.

### Task 11: Docs and headless scenes

**Files:**
- Modify: `leapp-rs/CLAUDE.md` (the new keys of Tasks 6–10; the `__new__` option; the
  `change_sessions` helper and its reservation), `crates/leapp-app/src/lib.rs` (`SCENES`:
  `settings-profiles`, `chained-dialog`, `choose-type`),
  `docs/superpowers/plans/2026-11-24-leapp-rs-f0-carry-forward.md` (mark the F2 items done with
  commit and test names; add anything this plan deferred under its feature).
- Test: `cargo run -p leapp-app -- --list-scenes` lists the three scenes; `--snap` of each writes
  a PNG under `target/shots/` (not committed). Compare by eye with
  `docs/images/screens/newuxui/aws-iam-role-chained.png` and `leapp-options.png`; note big
  differences in the report.

- [ ] **Step 1:** Add the scenes; run `--list-scenes` (expect the three names) and `--snap` each.
- [ ] **Step 2:** Update the docs.
- [ ] **Step 3:** `cargo test --workspace`, clippy and fmt → clean.
- [ ] **Step 4: Commit** `docs: F2 and F3 keys, scenes and carry-forward`.

### After Task 11

- The final whole-branch review on the most capable model, with rust-review, then one fix wave.
- The owner's check with real AWS (Tasks at a glance), then the finish menu.
