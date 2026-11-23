# Leapp RS

Rust and Rune rewrite of the Leapp AWS credentials app. The old TypeScript app is in the parent folder.

## Commands

Run all commands in `leapp-rs/`.

- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format check: `cargo fmt --check`
- Run the app: `cargo run -p leapp-app` (the crate comes in a later task)

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
- Until a crate uses Rune, Cargo warns that the patches are unused. This is fine.

## Gotchas

- `aws-sdk-sts` needs the `rt-tokio` feature. Without it, the client panics at build time.
- The pinned `aws-sdk-sts` uses the Query protocol (form body, `Action=...`).
  `fake-aws` answers in that protocol.
