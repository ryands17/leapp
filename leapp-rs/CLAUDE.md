# Leapp RS

Rust and Rune rewrite of the Leapp AWS credentials app. The old TypeScript app is in the parent folder.

## Commands

Run all commands in `leapp-rs/`.

- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format check: `cargo fmt --check`
- Run the app: `cargo run -p leapp-app` (`main.rs` is a stub until Task 8)

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
- `ScrollSpec` has no `Default`: write `ScrollSpec { stick_bottom: false }`.

## Gotchas

- `aws-sdk-sts` needs the `rt-tokio` feature. Without it, the client panics at build time.
- The pinned `aws-sdk-sts` uses the Query protocol (form body, `Action=...`).
  `fake-aws` answers in that protocol.
