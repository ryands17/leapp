# Leapp RS

Rust and Rune rewrite of the Leapp AWS credentials app. The old TypeScript app is in the parent folder.

## Commands

Run all commands in `leapp-rs/`.

- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format check: `cargo fmt --check`
- Run the app: `cargo run -p leapp-app`. It uses the real home folder, the real keychain
  (service `leapp-rs`) and real AWS. Close the window or press Cmd+Q to quit and clean up.
- Headless snapshots (no window, no real home, no keychain):
  `cargo run -p leapp-app -- --snap target/shots/main-empty.png`, add `--scene one-session`,
  `--scene create-dialog`, `create-dialog-region`, `create-dialog-profile`, `settings-profiles`,
  `chained-dialog` or `choose-type` for other states (`--list-scenes` lists them). Scenes are
  in `SCENES` in `crates/leapp-app/src/lib.rs`.

## Rules

- Tests never touch the real home folder, the real keychain, or real AWS.
  Use a `tempfile::TempDir` as home, `MemoryStore`, and `fake-aws`.
- Keep secrets in the `SecretStore` only. Never put them in `workspace.json` or logs.
- Secrets are secret access keys, session tokens, MFA codes and the user's backed-up section. They
  never go in `workspace.json`, logs, `Debug` output, error text or the element tree. The access key
  ID is an identifier (owner ruling): it may show in the UI and the element tree, but it is kept
  only in the `SecretStore` and never goes in `workspace.json`, logs, `Debug` output or error text.
- `leapp-core` must not depend on Rune.

## Rune

- Rune and rune-kit are git dependencies at rev `3d4e2dfcecb71890f7f4d6167e82fe6db9dbb695`.
- The root `Cargo.toml` patches `winit`, `sctk-adwaita`, `parley_engine`, `wgpu-hal`,
  `accesskit_windows` and `accesskit_macos` from the same git URL and rev.
- Update the Rune rev and all patches together. Cargo does not inherit patches from a dependency.
- UI tests use the headless harness in `crates/leapp-app/tests/common/mod.rs` (`AppHarness`).
  Element keys such as `row-<id>` and `status-<id>` are the test contract; keep them stable.
- `AppSpec::init` is a plain `fn`. It builds `Ui` from `leapp_app::install(..)`, which must run
  before `rune::run`.
- The headless runtime does not call `setup`, so the harness has no waker. `poll` runs every frame.
- rune-kit `Dialog::new(k, ..)` keys its panel `<k>-dialog`; `Select::new(k, ..)` keys its trigger `<k>-trigger`.
  The brief's keys (`dlg-iam-user`, `sel-region`) are wrapper elements around them.
- The region field is a rune-kit `Combobox` keyed `sel-region`: its trigger is
  `sel-region-value-trigger`, its options are `sel-region-option-<region>`, and typed text goes to
  its search field once it is open. Tests find a region by typing it, not by its list position.
- Every question (overwrite and MFA) ignores answers for `CONFIRM_ARM_SECONDS` (0.5 s of harness
  time) after it comes to the front, and its buttons are disabled until then. UI tests call
  `h.wait_for_arming()` before they tap `btn-overwrite-*` or `btn-mfa-*`.
- A rune-kit `Dialog` focuses its first Tab stop (the close button), and `TextInput` has no
  `autofocus`. To focus a field when a dialog opens, set `TextEdit::focused = true` in the model
  (the field needs `on_edit`).
- `ElementInfo` has no disabled flag. Read it from `rt.accessibility_json()`: the node keyed `k`
  has `"disabled"` in its `states` (`AppHarness::is_disabled`).
- A tap on a disabled button takes focus from a text field: tap the field again before you type.
- `Runtime::wake_deadline()` gives only the earliest wake of all sources (Rune has its own wakes), so
  a test cannot read one `wake_after` of `poll` from it.
- A `Select` list scrolls and its options can lie outside the window; tests pick the first options.
- `ScrollSpec` has no `Default`: write `ScrollSpec { stick_bottom: false }`.
- F1 keys: `row-menu-<id>` (the `⋯` button of a row), `menu-<id>` (the `Menu` key of its menu: the
  frame element is `menu-<id>-menu`, the items are `menu-<id>-item<i>`), `dlg-mfa`,
  `fld-mfa-code`, `btn-mfa-submit`, `btn-mfa-cancel`, `dlg-delete`, `btn-delete-yes`,
  `btn-delete-no`, `btn-secret-eye`, `fld-mfa-device`, `err-mfa-device`, `btn-empty-add-session`.
- F2 and F3 keys. Forms: `sel-profile` (a `Combobox`: the Named Profile select; the old text field
  `fld-profile` is gone), `sel-region`, `err-profile`, `err-region`. The option `__new__`
  (`sel-profile-option-__new__`) creates the typed name. The options of a select have the key
  `<select key>-option-<value>`: the region for `sel-region`, `sel-change-region` and
  `sel-default-region`, the profile name for `sel-profile` and `sel-change-profile`. Settings:
  `dlg-settings`, `tab-general`, `tab-profiles`, `sel-default-region`, `err-default-region`,
  `btn-settings-close`, `fld-new-profile`, `btn-profile-add`, `err-new-profile`,
  `btn-profile-rename-<id>`, `fld-profile-rename-<id>`, `btn-profile-rename-save-<id>`,
  `err-profile-rename-<id>`, `btn-profile-delete-<id>`, `dlg-delete-profile`,
  `btn-delete-profile-yes`, `btn-delete-profile-no`. Row menu items (by label): `Change region…`,
  `Change named profile…` and, for a chained session only, `Copy role ARN`. Change dialogs:
  `dlg-change-region`, `sel-change-region`, `btn-change-region-save`, `btn-change-region-cancel`,
  `err-change-region`, and the same keys with `-profile` (`dlg-change-profile`,
  `sel-change-profile`, ...). `dlg-chg-region` and `dlg-chg-profile` are only the rune-kit `Dialog`
  keys (panels `dlg-chg-region-dialog`, `dlg-chg-profile-dialog`). Type chooser: `dlg-choose-type`,
  `btn-type-iam-user`, `btn-type-iam-role-chained`. Chained form: `dlg-iam-role-chained`,
  `fld-chained-name`, `sel-parent` (a `Combobox`; its options are `sel-parent-option-<session id>`),
  `fld-role-arn`, `fld-role-session-name`, `fld-external-id`, `err-chained-name`, `err-parent`,
  `err-role-arn`, `err-role-session-name`, `err-external-id`, `err-chained-form`,
  `btn-chained-submit`, `btn-chained-cancel`.
- Core `change_sessions(ids, change)` (`sessions/change.rs`) is the one path for a change of many
  sessions (region, profile, rename or delete of a profile). It reserves the sessions with
  `Editing` (`sessions/edit.rs`) so that no start or refresh runs on them, stops the `Active` ones,
  saves, then starts them again. A session that another edit or change holds gives
  `ProfilePending`. Edit and delete of a chained session use the same `Editing` reservation.
  A create does not reserve: a create, or the save of an edit, whose parent an `Editing`
  reservation holds gives `ProfilePending` and changes nothing. A delete reserves every session
  below its target, so this also covers a delete of any session above the parent.
- Menu item keys hold an index, not a name. Tests pick an item by its label:
  `AppHarness::tap_menu_item(id, "Delete session")`.
- rune-kit `Menu::new(k)` keys its frame `<k>-menu` and its items `<k>-item<i>`. A chosen item does not close the menu: each `on_select` clears the app's open-menu state. `row-` also matches the `row-menu-<id>` buttons: tests list rows with `AppHarness::row_keys`.
- A revealed secure field (`TextInput::revealed(true)`) stays secure: `has_secure_focus()` is still true and the
  element tree has no text for it. A field's placeholder is only in `rt.accessibility_json()` (`AppHarness::placeholder_of`).

## Gotchas

- Headless runs (`rune::is_headless()`: `--snap`, `--tree`, `--script`, `--list-scenes`, `--frames`, ...)
  use a `tempfile::TempDir` home and a `MemoryStore`, so they never touch `~/.aws` or the keychain.
  `main.rs` keeps the `TempDir` alive for the whole run.
- Rune runs `App::on_close` for the close button, Cmd+Q and the Dock quit, so `leapp_app::on_close`
  calls `Leapp::shutdown` for all of them (`block_on` on the UI thread). `on_quit` is not used.
- Core has no timer. `main.rs` starts `leapp_app::spawn_refresh_loop` with `REFRESH_TICK` (10 s)
  after `startup`. Each tick runs in its own task (a panic ends only that tick). `on_close` aborts
  the loop. Two errors of one session in one tick go in one toast message, joined with `; `.
- The harness `Leapp` uses a `ManualClock` (`h.clock.advance(..)`, in `tests/common`; core's own
  test clock is not exported). `fake-aws` sets token expiry from the real time, so advance 36001 s
  to expire a token. `h.spawn_refresh_loop()` runs the loop with a 10 ms tick.
- If the keychain asks for a password at every build, the debug binary has a new signature each time.
  Sign it with an Apple Development certificate:
  `codesign -s "Apple Development: <name>" target/debug/leapp-app` (run it after each build).
- `save_parent_token` (`sessions/iam_user.rs`) writes the parent token to the keychain under
  the core state `Mutex` (no `.await` while it holds the lock), so that the key generation check
  and the write are one step. A keychain prompt then holds the lock, and the UI, which reads the
  workspace each frame, freezes until the user answers. F4 (SSO tokens) must decide on this
  (for example a write lock per parent) before it copies the pattern.

- `aws-sdk-sts` needs the `rt-tokio` feature. Without it, the client panics at build time.
- The pinned `aws-sdk-sts` uses the Query protocol (form body, `Action=...`).
  `fake-aws` answers in that protocol.
