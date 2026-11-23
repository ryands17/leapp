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
  `--scene create-dialog` or `--scene create-dialog-region` for other states
  (`--list-scenes` lists them). Scenes are in `SCENES` in `crates/leapp-app/src/lib.rs`.

## Rules

- Tests never touch the real home folder, the real keychain, or real AWS.
  Use a `tempfile::TempDir` as home, `MemoryStore`, and `fake-aws`.
- Keep secrets in the `SecretStore` only. Never put them in `workspace.json` or logs.
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
- The overwrite question ignores answers for `CONFIRM_ARM_SECONDS` (0.5 s of harness time) after it
  comes to the front. UI tests call `h.wait_for_arming()` before they tap `btn-overwrite-yes` or `-no`.
- `Runtime::wake_deadline()` gives only the earliest wake of all sources (Rune has its own wakes), so
  a test cannot read one `wake_after` of `poll` from it.
- A `Select` list scrolls and its options can lie outside the window; tests pick the first options.
- `ScrollSpec` has no `Default`: write `ScrollSpec { stick_bottom: false }`.

## Gotchas

- Headless runs (`rune::is_headless()`: `--snap`, `--tree`, `--script`, `--list-scenes`, `--frames`, ...)
  use a `tempfile::TempDir` home and a `MemoryStore`, so they never touch `~/.aws` or the keychain.
  `main.rs` keeps the `TempDir` alive for the whole run.
- Rune runs `App::on_close` for the close button, Cmd+Q and the Dock quit, so `leapp_app::on_close`
  calls `Leapp::shutdown` for all of them (`block_on` on the UI thread). `on_quit` is not used.
- If the keychain asks for a password at every build, the debug binary has a new signature each time.
  Sign it with an Apple Development certificate:
  `codesign -s "Apple Development: <name>" target/debug/leapp-app` (run it after each build).

- `aws-sdk-sts` needs the `rt-tokio` feature. Without it, the client panics at build time.
- The pinned `aws-sdk-sts` uses the Query protocol (form body, `Action=...`).
  `fake-aws` answers in that protocol.
