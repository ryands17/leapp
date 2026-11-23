# Leapp RS F0 (tracer bullet) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One IAM User session that you can add, start and stop in a Rune desktop app. Start writes temporary STS credentials to `~/.aws/credentials`. Stop removes them. Profiles that the user wrote by hand are protected.

**Architecture:** Cargo workspace `leapp-rs/` with three crates. `leapp-core` (library, no UI) owns the data, the keychain, STS calls and the credentials file. `fake-aws` (test-only) is a local HTTP server that acts as AWS STS. `leapp-app` (Rune) shows the data and calls `leapp-core` on a tokio runtime; results come back to the UI through a channel that the Rune `poll` hook drains.

**Tech Stack:** Rust 2024 edition (local toolchain 1.100), Rune + rune-kit at git rev `3d4e2dfcecb71890f7f4d6167e82fe6db9dbb695`, tokio, aws-config + aws-sdk-sts, keyring (apple-native), serde/serde_json, uuid, chrono, thiserror, async-trait, tempfile, axum (fake-aws only).

**Spec:** `docs/superpowers/specs/2026-11-23-leapp-rs-design.md`. Read sections 5, 6, 8, 9, 10.1, 10.2, 11.1, 12 and 16 (F0) before you start. Leapp reference code: `packages/core/src/services/session/aws/aws-session-service.ts` and `aws-iam-user-service.ts`.

## Global Constraints

- All code in `leapp-rs/` at the repo root. Branch `leapp-rs`.
- Rune and rune-kit as git dependencies at rev `3d4e2dfcecb71890f7f4d6167e82fe6db9dbb695`. The workspace root `Cargo.toml` must patch `winit`, `sctk-adwaita`, `parley_engine`, `wgpu-hal`, `accesskit_windows`, `accesskit_macos` from the same git URL and rev (Rune README, "Depending on Rune").
- `leapp-core` has no dependency on Rune.
- Secrets only in the `SecretStore` (keychain service name `leapp-rs`). Never in `workspace.json`, never in logs.
- Data file: `<home>/.leapp-rs/workspace.json`, `"version": 1`, atomic write (temp file in the same folder, then rename).
- Credentials file: `<home>/.aws/credentials`, mode `0600`; folder `<home>/.aws` mode `0700`. Section keys in this order: `aws_access_key_id`, `aws_secret_access_key`, `aws_session_token`, `region`.
- `GetSessionToken` with `DurationSeconds = 36000`. No MFA in F0.
- Default region for new sessions in F0: `us-east-1`. Default profile name: `default`.
- Keychain keys: `iam-user-<session_id>-access-key-id`, `iam-user-<session_id>-secret-access-key`, `iam-user-<session_id>-session-token`, `aws-profile-backup-<profile name>`.
- Tests never touch the real home folder, the real keychain or real AWS. Each test uses a `tempfile::TempDir` as home, `MemoryStore`, and `FakeAws`.
- Owner's test rules: E2E or integration tests through public interfaces; every test is a committed file; no tautological tests (each test must fail when its behavior breaks).
- One command runs all tests: `cd leapp-rs && cargo test --workspace`. Also clean: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`.
- Commit messages: Conventional Commits, ending with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- UI labels in ASD-STE100 Simplified Technical English where you write new text; copy Leapp labels where they exist.

## Review Focus

These inputs are not in the spec's F0 acceptance list, but a real user will meet them. Each one has a test in the task that owns the code.

1. **Credentials file with CRLF line endings or no final newline** → other bytes stay the same; our section is well formed and on its own lines. (Task 4: `keeps_crlf_and_missing_final_newline`)
2. **`~/.aws` folder or credentials file does not exist** → both are created, folder `0700`, file `0600`. (Task 3: `creates_aws_folder_and_file_with_private_modes`)
3. **The user's file has the same profile section two times** → start fails with `Error::DuplicateProfileSection`, file not changed. (Task 4: `duplicate_user_section_blocks_start`)
4. **STS returns an error** (bad keys, network) → status `Inactive`, file not changed, error text has the AWS message. (Task 3: `sts_error_leaves_file_unchanged`)
5. **Profile name that would break the file** (empty, only spaces, contains `[`, `]`, `\n`, `\r`) → create fails with `Error::InvalidInput { field: "profile" }`. (Task 2: `rejects_bad_profile_names`)

---

## File Structure

```
leapp-rs/
  Cargo.toml                    workspace, shared deps, [patch.crates-io] for Rune
  rust-toolchain.toml           channel = "1.100"
  CLAUDE.md                     commands, rules, gotchas
  .gitignore                    /target
  crates/
    leapp-core/
      Cargo.toml
      src/lib.rs                pub use; the `Leapp` facade
      src/config.rs             Config, Endpoints, Clock, SystemClock, Prompter
      src/error.rs              Error, Result
      src/model.rs              Workspace, Session, SessionKind, SessionStatus, NamedProfile, ManagedProfile, NewIamUser
      src/store.rs              load/save workspace.json
      src/secrets.rs            SecretStore, MemoryStore, KeychainStore
      src/aws_files.rs          credentials file text edits (section 9.3 of the spec)
      src/sessions/mod.rs       start, stop, startup, shutdown (10.1, 10.2)
      src/sessions/iam_user.rs  get credentials for IAM User (11.1, no MFA)
      src/regions.rs            REGIONS list
      tests/common/mod.rs       test harness (temp home, MemoryStore, FakeAws, scripted Prompter)
      tests/create_session.rs
      tests/start_stop.rs
      tests/user_profiles.rs
    fake-aws/
      Cargo.toml
      src/lib.rs                FakeAws server; STS GetSessionToken; request log; error injection
    leapp-app/
      Cargo.toml
      src/lib.rs                Ui state, view, poll, bridge (lib so tests can use it)
      src/bridge.rs             Msg, Bridge, UiPrompter
      src/views/mod.rs
      src/views/main_window.rs  sidebar + session table
      src/views/create_dialog.rs
      src/views/overwrite_dialog.rs
      src/main.rs               real app: keychain, real home, startup/shutdown
      tests/common/mod.rs       headless harness
      tests/main_window.rs
      tests/create_and_start.rs
```

## Shared Interfaces (defined in Tasks 1–4, used by Tasks 5–8)

```rust
// leapp-core
pub type SessionId = uuid::Uuid;

pub struct Config {
    pub home_dir: PathBuf,
    pub data_dir: PathBuf,                 // default home_dir/.leapp-rs
    pub secrets: Arc<dyn SecretStore>,
    pub endpoints: Endpoints,
    pub clock: Arc<dyn Clock>,
    pub prompter: Arc<dyn Prompter>,
}
impl Config {
    pub fn new(home_dir: PathBuf, secrets: Arc<dyn SecretStore>, prompter: Arc<dyn Prompter>) -> Config; // real endpoints, SystemClock
}
#[derive(Clone, Default)] pub struct Endpoints { pub sts: Option<String> }   // None = real AWS
pub trait Clock: Send + Sync { fn now(&self) -> DateTime<Utc>; }
pub struct SystemClock;
#[async_trait] pub trait Prompter: Send + Sync {
    async fn confirm_profile_overwrite(&self, profile: &str) -> bool;
}
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;   // Ok if missing
}
#[derive(Default)] pub struct MemoryStore { /* Mutex<BTreeMap<String,String>> */ }
impl MemoryStore { pub fn snapshot(&self) -> BTreeMap<String, String>; }
pub struct KeychainStore; impl KeychainStore { pub fn new(service: &str) -> Self; }

pub struct Leapp;   // Send + Sync; wrap in Arc to share
impl Leapp {
    pub fn open(config: Config) -> Result<Leapp>;
    pub fn workspace(&self) -> Workspace;                       // clone snapshot
    pub fn create_iam_user(&self, req: NewIamUser) -> Result<SessionId>;
    pub async fn start(&self, id: SessionId) -> Result<()>;
    pub async fn stop(&self, id: SessionId) -> Result<()>;
    pub async fn startup(&self) -> Result<()>;   // stop all; clean every managed profile
    pub async fn shutdown(&self) -> Result<()>;  // same cleanup as startup
}
pub struct NewIamUser { pub name: String, pub access_key_id: String, pub secret_access_key: String,
                        pub profile_name: String, pub region: String }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace { pub version: u32, pub sessions: Vec<Session>, pub profiles: Vec<NamedProfile>,
                       pub managed_profiles: BTreeMap<String, ManagedProfile> }
impl Workspace { pub fn session(&self, id: SessionId) -> Option<&Session>;
                 pub fn profile_name(&self, profile_id: Uuid) -> Option<&str>; }
pub struct Session { pub id: SessionId, pub name: String, pub region: String, pub profile_id: Uuid,
                     pub status: SessionStatus, pub start_time: Option<DateTime<Utc>>,
                     pub session_token_expiration: Option<DateTime<Utc>>, pub kind: SessionKind }
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKind { IamUser { mfa_device: Option<String>, account: Option<String> } }
#[serde(rename_all = "snake_case")] pub enum SessionStatus { Inactive, Pending, Active }
pub struct NamedProfile { pub id: Uuid, pub name: String }
pub struct ManagedProfile { pub session_id: SessionId, pub had_original: bool }

#[derive(thiserror::Error, Debug)]
pub enum Error {
    NotFound(SessionId),
    InvalidInput { field: &'static str, message: String },
    Aws(String),                       // message from AWS, shown to the user
    ProfileOverwriteDeclined(String),  // profile name
    ProfilePending(String),            // "Another session with the same named profile is starting"
    DuplicateProfileSection(String),
    UnsupportedVersion(u32),
    Secret(String),
    Io(#[from] std::io::Error),
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub mod regions { pub const REGIONS: &[&str]; pub const DEFAULT_REGION: &str = "us-east-1"; }

// fake-aws
pub struct FakeAws;
impl FakeAws {
    pub async fn start() -> FakeAws;                     // binds 127.0.0.1:0
    pub fn url(&self) -> String;                         // "http://127.0.0.1:<port>"
    pub fn requests(&self) -> Vec<Recorded>;
    pub fn count(&self, action: &str) -> usize;
    pub fn fail_next(&self, action: &str, status: u16, code: &str, message: &str);
    pub fn delay_next(&self, action: &str, delay: std::time::Duration);   // the next call of `action` waits first
}
pub struct Recorded { pub action: String, pub params: BTreeMap<String, String>,
                      pub access_key_id: Option<String> }   // from the SigV4 Authorization header
```

---

### Task 1: Workspace scaffold and the fake STS server

**Files:**
- Create: `leapp-rs/Cargo.toml`, `leapp-rs/rust-toolchain.toml`, `leapp-rs/.gitignore`, `leapp-rs/CLAUDE.md`
- Create: `leapp-rs/crates/fake-aws/Cargo.toml`, `leapp-rs/crates/fake-aws/src/lib.rs`
- Create: `leapp-rs/crates/leapp-core/Cargo.toml`, `leapp-rs/crates/leapp-core/src/lib.rs` (empty module list for now)
- Test: `leapp-rs/crates/fake-aws/tests/sts.rs`

**Interfaces:**
- Produces: `FakeAws` (see Shared Interfaces).

- [ ] **Step 1: Create the workspace.** `members = ["crates/*"]`, `resolver = "3"`, `[workspace.package] edition = "2024"`, `[workspace.dependencies]` for every crate in the Tech Stack line. `leapp-app` is added in Task 5, so the glob must tolerate it not existing yet (it does). Add `[patch.crates-io]` now (see Global Constraints) so the lock file is right from the start. `CLAUDE.md` lists: the test, clippy, fmt and run commands; the rule "tests never touch real home, keychain or AWS"; the Rune rev and "update all patches together".

- [ ] **Step 2: Write the failing test** `crates/fake-aws/tests/sts.rs`:

```rust
#[tokio::test]
async fn get_session_token_returns_credentials_and_records_the_call() {
    let fake = FakeAws::start().await;
    let conf = aws_sdk_sts::Config::builder()
        .behavior_version(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_sts::config::Region::new("eu-west-1"))
        .endpoint_url(fake.url())
        .credentials_provider(aws_sdk_sts::config::Credentials::new("AKIDLONG", "secret", None, None, "test"))
        .build();
    let out = aws_sdk_sts::Client::from_conf(conf)
        .get_session_token().duration_seconds(36000).send().await.unwrap();
    let c = out.credentials().unwrap();
    assert!(c.access_key_id().starts_with("ASIAFAKE"));
    assert_eq!(fake.count("GetSessionToken"), 1);
    let r = &fake.requests()[0];
    assert_eq!(r.params.get("DurationSeconds").map(String::as_str), Some("36000"));
    assert_eq!(r.access_key_id.as_deref(), Some("AKIDLONG"));
}

#[tokio::test]
async fn fail_next_returns_an_aws_error_once() {
    // fail_next("GetSessionToken", 403, "InvalidClientTokenId", "The security token included in the request is invalid.")
    // first call: Err whose display text contains "InvalidClientTokenId"; second call: Ok
}
```

- [ ] **Step 3: Run it.** `cd leapp-rs && cargo test -p fake-aws` → FAIL (`FakeAws` not defined).

- [ ] **Step 4: Implement `FakeAws`** in `crates/fake-aws/src/lib.rs` with axum on `127.0.0.1:0`, in a tokio task. First log one raw request from the SDK to see its protocol. The pinned `aws-sdk-sts` sends either the Query protocol (`application/x-www-form-urlencoded` body, `Action=GetSessionToken`) or the AWS JSON protocol (`X-Amz-Target: AWSSecurityTokenServiceV20110615.GetSessionToken`). Support the one it sends, and record params into `Recorded.params` the same way for both. The access key ID comes from the `Authorization` header (`Credential=<AKID>/...`). Credentials it returns: number `n` counts from 1 for each call: `ASIAFAKE{n:04}`, `fake-secret-{n}`, `fake-token-{n}`, expiration = now + `DurationSeconds`. Query-protocol success body:

```xml
<GetSessionTokenResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/">
  <GetSessionTokenResult><Credentials>
    <AccessKeyId>{id}</AccessKeyId><SecretAccessKey>{secret}</SecretAccessKey>
    <SessionToken>{token}</SessionToken><Expiration>{rfc3339}</Expiration>
  </Credentials></GetSessionTokenResult>
  <ResponseMetadata><RequestId>fake-{n}</RequestId></ResponseMetadata>
</GetSessionTokenResponse>
```

Query-protocol error body (with the injected status):

```xml
<ErrorResponse><Error><Type>Sender</Type><Code>{code}</Code><Message>{message}</Message></Error><RequestId>fake-err</RequestId></ErrorResponse>
```

- [ ] **Step 5: Run it.** `cargo test -p fake-aws` → PASS (2 tests).

- [ ] **Step 6: Commit.** `git add leapp-rs && git commit -m "build(leapp-rs): add cargo workspace and fake STS server"`

---

### Task 2: Data model, store and create IAM User

**Files:**
- Create: `crates/leapp-core/src/{config.rs,error.rs,model.rs,store.rs,secrets.rs,regions.rs}`; modify `src/lib.rs`
- Test: `crates/leapp-core/tests/common/mod.rs`, `crates/leapp-core/tests/create_session.rs`

**Interfaces:**
- Produces: `Config`, `Endpoints`, `Clock`, `SystemClock`, `Prompter`, `SecretStore`, `MemoryStore`, `KeychainStore`, `Leapp::open`, `Leapp::workspace`, `Leapp::create_iam_user`, all model types, `Error`, `regions::REGIONS`, `regions::DEFAULT_REGION`.
- Test harness (`tests/common/mod.rs`), used by Tasks 3 and 4:

```rust
pub struct Harness { pub home: TempDir, pub secrets: Arc<MemoryStore>, pub prompter: Arc<ScriptedPrompter>,
                     pub fake: FakeAws, pub leapp: Leapp }
impl Harness {
    pub async fn new() -> Harness;              // Config with endpoints.sts = Some(fake.url())
    pub fn reopen(self) -> Harness;             // drop Leapp WITHOUT shutdown, open a new one on the same dirs
    pub fn credentials_text(&self) -> Option<String>;   // <home>/.aws/credentials
    pub fn write_credentials(&self, text: &str);
    pub fn workspace_text(&self) -> String;     // <home>/.leapp-rs/workspace.json
    pub fn iam_user(&self, name: &str, profile: &str) -> NewIamUser; // keys "AKID-<name>", "SECRET-<name>", region "eu-west-1"
}
pub struct ScriptedPrompter { /* answer: AtomicBool, asked: Mutex<Vec<String>> */ }
impl ScriptedPrompter { pub fn answer(&self, yes: bool); pub fn asked(&self) -> Vec<String>; }
```

- [ ] **Step 1: Write the failing tests** in `tests/create_session.rs`:

```rust
#[tokio::test] async fn created_session_is_saved_and_survives_reopen() {
    // create_iam_user(h.iam_user("dev", "default")) → id
    // workspace(): one session, name "dev", region "eu-west-1", status Inactive, kind IamUser{mfa_device: None, ..},
    //              profile_name(session.profile_id) == Some("default")
    // h.reopen(): same session is there
}
#[tokio::test] async fn keys_go_to_the_secret_store_not_the_file() {
    // after create: secrets.snapshot()["iam-user-<id>-access-key-id"] == "AKID-dev", "...-secret-access-key" == "SECRET-dev"
    // workspace_text() does not contain "AKID-dev" and does not contain "SECRET-dev"
}
#[tokio::test] async fn new_profile_name_creates_a_profile_once() {
    // two sessions with profile "ci" → workspace().profiles has exactly "default" and "ci" (one "ci")
}
#[tokio::test] async fn default_profile_exists_in_a_new_workspace() {
    // fresh Harness: profiles == ["default"]
}
#[tokio::test] async fn rejects_bad_profile_names() {
    // for p in ["", "   ", "a[b", "a]b", "a\nb", "a\rb"]: Err(Error::InvalidInput { field: "profile", .. })
}
#[tokio::test] async fn rejects_missing_fields_and_unknown_region() {
    // empty name → field "name"; empty access key → "access_key_id"; empty secret → "secret_access_key";
    // region "mars-1" → field "region"
}
#[tokio::test] async fn newer_version_file_is_refused_and_not_overwritten() {
    // write {"version": 99, ...} to workspace.json; Leapp::open → Err(Error::UnsupportedVersion(99)); file bytes unchanged
}
#[tokio::test] async fn broken_json_file_is_moved_aside() {
    // write "{not json" → open Ok with empty workspace; a file "workspace.json.broken-*" exists with the old bytes
}
```

- [ ] **Step 2: Run them.** `cargo test -p leapp-core --test create_session` → FAIL (compile errors).

- [ ] **Step 3: Implement.** Model types and serde as in Shared Interfaces and spec section 12 (only the F0 fields). `Leapp` keeps `Mutex<Workspace>` (std mutex; never hold it across `.await`) and saves after each change. `store.rs`: `load(path) -> Result<Workspace>` (missing file = new workspace with profile `default`), `save(path, &Workspace)` (atomic via `tempfile::NamedTempFile::new_in(dir)` + `persist`). Trim `name`, keys and profile name; region must be in `REGIONS` (copy the list from `packages/core/src/services/aws-core-service.ts:320`). `KeychainStore` uses `keyring::Entry::new("leapp-rs", key)`; `NoEntry` on get → `Ok(None)`, on delete → `Ok(())`. `KeychainStore` has no automated test (real keychain); Task 8 checks it by hand.

- [ ] **Step 4: Run them.** `cargo test -p leapp-core --test create_session` → PASS (8 tests).

- [ ] **Step 5: Commit.** `git commit -m "feat(leapp-core): add workspace store, secret store and IAM user creation"`

---

### Task 3: Start and stop an IAM User session

**Files:**
- Create: `crates/leapp-core/src/aws_files.rs`, `src/sessions/mod.rs`, `src/sessions/iam_user.rs`
- Test: `crates/leapp-core/tests/start_stop.rs`

**Interfaces:**
- Consumes: Task 2 types and harness.
- Produces: `Leapp::start`, `Leapp::stop`. Internal `aws_files` API that Task 4 extends:

```rust
pub(crate) struct CredentialsFile { path: PathBuf }
impl CredentialsFile {
    pub fn new(home: &Path) -> Self;
    pub fn sections_named(&self, profile: &str) -> Result<usize>;            // count of [profile] headers (trimmed match)
    pub fn section_text(&self, profile: &str) -> Result<Option<String>>;     // full text incl. header, as in the file
    pub fn write_section(&self, profile: &str, body: &SectionBody) -> Result<()>;  // replace in place or append
    pub fn write_raw_section(&self, profile: &str, text: &str) -> Result<()>;      // restore a saved original
    pub fn remove_section(&self, profile: &str) -> Result<()>;
}
pub(crate) struct SectionBody { pub access_key_id: String, pub secret_access_key: String,
                                pub session_token: String, pub region: String }
```

- [ ] **Step 1: Write the failing tests** in `tests/start_stop.rs`:

```rust
#[tokio::test] async fn start_writes_the_profile_section_with_temporary_credentials() {
    // create "dev" on "default", start → status Active, start_time Some
    // credentials_text() == "[default]\naws_access_key_id = ASIAFAKE0001\naws_secret_access_key = fake-secret-1\naws_session_token = fake-token-1\nregion = eu-west-1\n"
    // fake: one GetSessionToken, DurationSeconds "36000", access_key_id "AKID-dev"
}
#[tokio::test] async fn stop_removes_the_section_and_sets_inactive() {
    // start, stop → credentials_text() == Some("") ; status Inactive; start_time None
}
#[tokio::test] async fn other_sections_and_comments_stay_byte_identical() {
    // write "# my notes\n[work]\naws_access_key_id = X\n\n[other] ; c\nregion = us-west-2\n"
    // start session on "dev-profile" → file == original + "[dev-profile]\n" + the 4 key lines, each ending "\n"
    //   (original already ends with "\n", so nothing is added before the header)
    // stop → text equals the original exactly
}
#[tokio::test] async fn creates_aws_folder_and_file_with_private_modes() {
    // no ~/.aws; start → folder mode & 0o777 == 0o700, file mode & 0o777 == 0o600
}
#[tokio::test] async fn token_is_cached_until_it_expires() {
    // start, stop, start → fake.count("GetSessionToken") == 1; second write uses ASIAFAKE0001 again
    // cached token in secrets under "iam-user-<id>-session-token"; session_token_expiration is Some
}
#[tokio::test] async fn second_session_on_same_profile_stops_the_first() {
    // A and B on "default"; start A, start B → A Inactive, B Active; file has B's keys (ASIAFAKE0002) once
}
#[tokio::test] async fn sts_error_leaves_file_unchanged() {
    // write "[work]\nx = 1\n"; fail_next("GetSessionToken", 403, "InvalidClientTokenId", "bad key")
    // start → Err(Error::Aws(m)) with m containing "bad key"; status Inactive; file text unchanged
}
#[tokio::test] async fn workspace_file_never_contains_temporary_credentials() {
    // after start: workspace_text() contains none of "ASIAFAKE0001", "fake-secret-1", "fake-token-1"
}
#[tokio::test] async fn stop_of_an_inactive_session_does_nothing() {
    // write "[default]\nmine = 1\n" (user's own); stop(id of inactive session) → Ok, file unchanged
}
```

- [ ] **Step 2: Run them.** `cargo test -p leapp-core --test start_stop` → FAIL.

- [ ] **Step 3: Implement `aws_files.rs`.** Text-based edit, no INI library. A section starts at a line whose trimmed text matches `[name]` (compare `name.trim()`), and ends before the next header line or at end of file. Keep the file's line ending (`\r\n` if the first line break is `\r\n`, else `\n`). Write with keys `key = value`. Replace in place; append at the end with one line break before the header if the file does not end with one. Atomic write (temp file in `~/.aws`, set mode `0600`, rename). Create `~/.aws` with mode `0700` if missing.

- [ ] **Step 4: Implement start/stop** in `sessions/mod.rs` and `sessions/iam_user.rs`, following spec 10.1, 10.2 and 11.1 (no MFA). `iam_user::credentials(&self, session) -> Result<SectionBody-like creds>`: cached token from the secret store if `session_token_expiration > clock.now()`; else STS `GetSessionToken` (client with `endpoint_url` from `Endpoints.sts` when set; region = session region; static credentials from the secret store; `timeout_config` operation timeout 10 s; retry `max_attempts(1)`); save token JSON and expiration. Trim all values. In this task, `managed_profiles` gets `{session_id, had_original: false}` on start and loses the entry on stop. The user-written-profile check comes in Task 4; until then, treat an existing unmanaged section as replaceable.

- [ ] **Step 5: Run them.** `cargo test -p leapp-core --test start_stop` → PASS (9 tests). Also run the full suite: `cargo test --workspace` → PASS.

- [ ] **Step 6: Commit.** `git commit -m "feat(leapp-core): start and stop IAM user sessions in the credentials file"`

---

### Task 4: Protect user-written profiles; startup and shutdown cleanup

**Files:**
- Modify: `crates/leapp-core/src/aws_files.rs`, `src/sessions/mod.rs`, `src/lib.rs`
- Test: `crates/leapp-core/tests/user_profiles.rs`

**Interfaces:**
- Consumes: Task 3 `CredentialsFile`.
- Produces: `Leapp::startup`, `Leapp::shutdown`; the prompt rule of spec 9.3 step 5.

- [ ] **Step 1: Write the failing tests** in `tests/user_profiles.rs`:

```rust
const MINE: &str = "[default]\naws_access_key_id = AKIAUSER\naws_secret_access_key = usersecret\n";

#[tokio::test] async fn declined_overwrite_fails_and_keeps_the_file() {
    // write MINE; prompter.answer(false); start → Err(ProfileOverwriteDeclined("default")); status Inactive
    // prompter.asked() == ["default"]; file == MINE; fake.count("GetSessionToken") == 0
}
#[tokio::test] async fn accepted_overwrite_saves_the_original_and_restores_it_on_stop() {
    // write MINE; answer(true); start → file has ASIAFAKE0001 under [default], not AKIAUSER
    // secrets["aws-profile-backup-default"] == MINE; workspace().managed_profiles["default"].had_original == true
    // workspace_text() does not contain "usersecret"
    // stop → file == MINE; backup key gone; managed_profiles empty
}
#[tokio::test] async fn own_managed_profile_does_not_ask_again() {
    // no user section; start A on default; start B on default → prompter.asked() is empty
}
#[tokio::test] async fn duplicate_user_section_blocks_start() {
    // write "[default]\na = 1\n[default]\nb = 2\n"; start → Err(DuplicateProfileSection("default")); file unchanged; no prompt
}
#[tokio::test] async fn keeps_crlf_and_missing_final_newline() {
    // write "[work]\r\nx = 1" (no final newline); start on "default"
    // while active: file starts with "[work]\r\nx = 1\r\n[default]\r\n"; no "\n" without a "\r" before it
    // stop → file == "[work]\r\nx = 1\r\n" (the line break added before our header stays; nothing else changes)
}
#[tokio::test] async fn second_start_on_a_pending_profile_is_refused() {
    // A and B on "default"; fake.delay_next("GetSessionToken", 300 ms)
    // tokio::join!(start(A), async { sleep(50 ms); start(B) }) → A Ok, B Err(ProfilePending(_)); A Active, B Inactive
}
#[tokio::test] async fn startup_after_crash_removes_our_sections() {
    // start (no user section); h.reopen() (no shutdown); startup() → credentials has no [default]; session Inactive
}
#[tokio::test] async fn startup_after_crash_restores_the_user_section() {
    // write MINE; answer(true); start; h.reopen(); startup() → file == MINE; backup key gone
}
#[tokio::test] async fn shutdown_cleans_all_active_sessions() {
    // sessions on "default" and "ci", both started; shutdown() → file has neither section; both Inactive
}
```

- [ ] **Step 2: Run them.** `cargo test -p leapp-core --test user_profiles` → FAIL.

- [ ] **Step 3: Implement** spec 9.3 steps 4 to 7. Order in `start`: (1) pending-profile check (10.1 step 1, `Error::ProfilePending`), (2) stop other active sessions on the same profile, (3) if the profile is not in `managed_profiles`: count sections; `> 1` → `DuplicateProfileSection`; `== 1` → ask the prompter; no → `ProfileOverwriteDeclined`; yes → store `section_text` in the secret store under `aws-profile-backup-<profile>` and remember `had_original = true`, (4) set `Pending`, (5) get credentials, (6) write the section, (7) save `managed_profiles` and status `Active`. If step 5 or 6 fails after a backup was saved, restore the original section and delete the backup. `startup()` and `shutdown()`: for each managed profile, restore or remove (9.3 step 6), then set all sessions `Inactive` with `start_time = None`, then save.

- [ ] **Step 4: Run them.** `cargo test --workspace` → PASS (all core and fake-aws tests).

- [ ] **Step 5: Commit.** `git commit -m "feat(leapp-core): protect user-written profiles and clean up on startup and shutdown"`

---

### Task 5: Rune app skeleton with the main window

**Files:**
- Create: `crates/leapp-app/Cargo.toml` (deps: `rune`, `rune-kit` git rev from Global Constraints with `default-features = true`; `leapp-core`; `tokio`; dev-deps `fake-aws`, `tempfile`)
- Create: `crates/leapp-app/src/{lib.rs,bridge.rs,views/mod.rs,views/main_window.rs,main.rs}`
- Test: `crates/leapp-app/tests/common/mod.rs`, `crates/leapp-app/tests/main_window.rs`

**Interfaces:**
- Consumes: `Leapp`, `Workspace`, `Session`, `SessionStatus`.
- Produces:

```rust
// lib.rs
pub struct Ui { /* leapp: Arc<Leapp>, rt: tokio::runtime::Handle, bridge: Bridge, ws: Workspace, error: Option<String>, ... */ }
impl Ui { pub fn new(leapp: Arc<Leapp>, rt: tokio::runtime::Handle, bridge: Bridge) -> Ui; }
pub fn view(s: &Ui, cx: &mut rune::Cx<Ui>) -> rune::El;
pub fn poll(s: &mut Ui, t: f64, tx: &mut rune::Tx) -> bool;   // drains bridge; true if state changed
pub fn setup(s: &mut Ui, waker: rune::Waker);                 // stores the waker in the bridge
pub fn spec() -> rune::AppSpec<Ui>;                           // title "Leapp RS", size (1100, 700), setup, poll
// bridge.rs
pub enum Msg { Refreshed, Failed(String), ConfirmOverwrite { profile: String, reply: tokio::sync::oneshot::Sender<bool> } }
pub struct Bridge;   // Clone-able sender half + receiver; holds Arc<Mutex<Option<Waker>>>
impl Bridge { pub fn new() -> Bridge; pub fn send(&self, m: Msg); }  // send() calls the waker if set
pub struct UiPrompter; impl UiPrompter { pub fn new(bridge: Bridge) -> Self; }  // impl leapp_core::Prompter
impl Ui { pub fn run<F>(&mut self, f: F) where F: Future<Output = leapp_core::Result<()>> + Send + 'static; }
    // spawns f on rt; on Ok sends Refreshed, on Err sends Failed(err.to_string())
```

- Element keys (tests depend on them): `sidebar`, `nav-all-sessions`, `table`, `row-<id>`, `cell-name-<id>`, `cell-provider-<id>` (text `IAM User`), `cell-profile-<id>`, `cell-region-<id>` (uppercase, e.g. `EU-WEST-1`), `status-<id>` (label `inactive` / `pending` / `active`), `empty-state`, `toast-error`.
- Headless harness (`tests/common/mod.rs`), used by Tasks 6 and 7:

```rust
pub struct AppHarness { pub home: TempDir, pub fake: FakeAws, pub secrets: Arc<MemoryStore>,
                        pub leapp: Arc<Leapp>, pub rt: rune::Runtime<Ui>, tokio: tokio::runtime::Runtime }
impl AppHarness {
    pub fn new() -> AppHarness;                       // 1100x700 viewport; Runtime::new(Ui::new(..), view, ..); rt.poll = Some(poll)
    pub fn frames(&mut self, n: usize);               // rt.frame(1.0 / 60.0) n times
    pub fn settle(&mut self, until: impl Fn(&Ui) -> bool);   // frames + 5 ms sleeps, panics after 5 s real time
    pub fn tap(&mut self, key: &str);                 // find + pointer_down + pointer_up + frames(2); panics if key missing
    pub fn type_text(&mut self, text: &str);          // rt.key(KeyEvent { key: Key::Text(..), mods: Mods::default() })
    pub fn text_of(&self, key: &str) -> Option<String>;   // from rt.elements(): text, or label if no text, of that key or its first text child
    pub fn has(&self, key: &str) -> bool;
}
```

- [ ] **Step 1: Write the failing tests** in `tests/main_window.rs`:

```rust
#[test] fn empty_workspace_shows_the_empty_state() {
    // AppHarness::new(); frames(2) → has("empty-state"); !has("table") or table has no rows
}
#[test] fn existing_sessions_show_as_rows() {
    // before Ui::new: leapp.create_iam_user("dev", profile "ci", region "eu-west-1") → id
    // text_of("cell-name-<id>") == "dev"; cell-provider "IAM User"; cell-profile "ci";
    // cell-region "EU-WEST-1"; text_of("status-<id>") == "inactive"
}
#[test] fn active_sessions_are_listed_first() {
    // create "a-first" and "b-second" (different profiles); start "b-second" through leapp; trigger a refresh
    // (bridge.send(Msg::Refreshed)); settle; row-<b> y position < row-<a> y position (rect[1] from elements())
}
```

- [ ] **Step 2: Run them.** `cargo test -p leapp-app --test main_window` → FAIL.

- [ ] **Step 3: Implement.** Layout from spec 14.1 and `docs/images/screens/newuxui/main-window.png`: left sidebar (width about 220 pt) with "All Sessions"; right side toolbar (only the (+) button with key `btn-create`, used in Task 6) and a table. Use `rune_kit::Kit::neutral` (or `Kit::from_palette`) for colors; rune-kit `Table` if it fits, else rows of fixed-width columns. Provider chip for IAM User: orange. Region chip: grey. Status dot: grey (inactive), spinner or pulsing (pending), orange (active). `poll` drains the bridge: `Refreshed` → `s.ws = s.leapp.workspace()`; `Failed(m)` → `s.error = Some(m)` and refresh; `ConfirmOverwrite` is handled in Task 7 (store it in `Ui` now; the view ignores it). Keep `main.rs` as a stub that panics with "see Task 8" so the bin compiles.

- [ ] **Step 4: Run them.** `cargo test -p leapp-app --test main_window` → PASS. Also add a scene `one-session` to `spec()` (it fills `Ui` with one inactive IAM User row and needs no AWS); Task 8 renders it.

- [ ] **Step 5: Commit.** `git commit -m "feat(leapp-app): show sessions in the main window"`

---

### Task 6: Create IAM User dialog

**Files:**
- Create: `crates/leapp-app/src/views/create_dialog.rs`; modify `lib.rs`, `views/main_window.rs`
- Test: `crates/leapp-app/tests/create_and_start.rs` (first two tests)

**Interfaces:**
- Consumes: `Leapp::create_iam_user`, `regions::REGIONS`, `regions::DEFAULT_REGION`, `AppHarness`.
- Produces element keys: `btn-create`, `dlg-iam-user`, `fld-name`, `fld-access-key`, `fld-secret-key` (secure field), `fld-profile` (text, prefilled `default`), `sel-region` (rune-kit `Select`, preselected `us-east-1`), `btn-create-submit`, `btn-create-cancel`, `err-<field>` (validation text under a field).

- [ ] **Step 1: Write the failing tests:**

```rust
#[test] fn create_dialog_adds_a_session() {
    // tap btn-create → has("dlg-iam-user")
    // tap fld-name, type "dev"; tap fld-access-key, type "AKID-dev"; tap fld-secret-key, type "SECRET-dev"
    // tap btn-create-submit; settle(|ui| ui has 1 session)
    // !has("dlg-iam-user"); one row; cell-name "dev"; cell-profile "default"; cell-region "US-EAST-1"
    // secrets snapshot has "iam-user-<id>-secret-access-key" == "SECRET-dev"
}
#[test] fn create_is_blocked_until_required_fields_are_set() {
    // open dialog; tap btn-create-submit with empty fields → dialog still open; no session created
    // text_of("err-name") is Some(..)
}
```

- [ ] **Step 2: Run them.** `cargo test -p leapp-app --test create_and_start` → FAIL.

- [ ] **Step 3: Implement** with rune-kit `Dialog`, `TextInput` (secure for the secret), `Select`, `Button`. Labels from Leapp (`docs/images/screens/newuxui/aws-iam-user.png`): "Session Alias", "AWS Access Key ID", "AWS Secret Access Key", "Named Profile", "Region". Submit calls `leapp.create_iam_user` (it is sync; call it in the handler, it only writes small files) and maps `Error::InvalidInput { field, message }` to `err-<field>`. Close the dialog and clear the field values (secure fields wipe on clear) on success.

- [ ] **Step 4: Run them.** PASS.

- [ ] **Step 5: Commit.** `git commit -m "feat(leapp-app): add the create IAM user dialog"`

---

### Task 7: Start and stop from the UI; overwrite confirm; errors

**Files:**
- Create: `crates/leapp-app/src/views/overwrite_dialog.rs`; modify `lib.rs`, `bridge.rs`, `views/main_window.rs`
- Test: `crates/leapp-app/tests/create_and_start.rs` (more tests)

**Interfaces:**
- Consumes: `Leapp::start`, `Leapp::stop`, `Ui::run`, `UiPrompter`, `Msg::ConfirmOverwrite`.
- Produces element keys: `dlg-overwrite` (text names the profile), `btn-overwrite-yes`, `btn-overwrite-no`; `toast-error`.
- Interaction: tap `status-<id>` or double-click `row-<id>` toggles start/stop. While a start runs, the UI shows `pending` at once (set locally before `run`), not after the round trip.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test] fn start_and_stop_from_the_table() {
    // create session via leapp; tap status-<id> → text_of("status-<id>") == "pending" in the same or next frame
    // settle(active) → credentials file has [default] with ASIAFAKE0001
    // tap status-<id>; settle(inactive) → no [default] section
}
#[test] fn overwrite_prompt_yes_replaces_and_stop_restores() {
    // write "[default]\naws_access_key_id = AKIAUSER\n" to the temp home; tap status-<id>
    // settle(has("dlg-overwrite")); text_of("dlg-overwrite") contains "default"
    // tap btn-overwrite-yes; settle(active) → file has ASIAFAKE0001; tap status; settle(inactive) → file is the original
}
#[test] fn overwrite_prompt_no_shows_an_error_and_keeps_the_file() {
    // same setup; tap btn-overwrite-no; settle(has("toast-error")) → status "inactive"; file unchanged
}
#[test] fn aws_error_is_shown_to_the_user() {
    // fake.fail_next("GetSessionToken", 403, "InvalidClientTokenId", "The security token included in the request is invalid.")
    // tap status-<id>; settle(has("toast-error")) → text contains "security token"; status "inactive"
}
```

- [ ] **Step 2: Run them.** FAIL.

- [ ] **Step 3: Implement.** `UiPrompter::confirm_profile_overwrite` sends `Msg::ConfirmOverwrite` with a oneshot sender and awaits the reply (dropped sender = `false`). `poll` stores the pending request in `Ui`; the dialog's buttons send the answer and clear it. Error toast: text from `Failed(m)`, a close button, auto-hide after 8 s using the `t` argument of `poll`. Use `Ui::run` for start and stop.

- [ ] **Step 4: Run them.** `cargo test --workspace` → PASS.

- [ ] **Step 5: Commit.** `git commit -m "feat(leapp-app): start and stop sessions with profile overwrite confirm"`

---

### Task 8: Real app entry point and owner demo

**Files:**
- Modify: `crates/leapp-app/src/main.rs`, `leapp-rs/CLAUDE.md`

**Interfaces:**
- Consumes: `spec()`, `Ui::new`, `KeychainStore::new("leapp-rs")`, `UiPrompter`, `Leapp::startup`, `Leapp::shutdown`.

- [ ] **Step 1: Implement `main`.** Build a multi-thread tokio runtime and keep it alive for the whole process. Home = `dirs::home_dir()` (or `std::env::home_dir()`). `Config::new(home, Arc::new(KeychainStore::new("leapp-rs")), Arc::new(UiPrompter::new(bridge.clone())))`. `rt.block_on(leapp.startup())` before the window opens (log and show the error if it fails; do not abort). `rune::App::new(spec()).init(move || Ui::new(..)).on_close(|s, tx| { block_on(s.leapp.shutdown()); tx.quit(); })`. In F0, closing the window quits (menu-bar mode is F8). Also run `shutdown` on Cmd+Q if Rune routes it through a different path (check `App::on_close` docs in the Rune README "App lifetime").

- [ ] **Step 2: Headless render check.** `cd leapp-rs && cargo run -p leapp-app -- --snap target/shots/main-empty.png` and `-- --snap target/shots/main-one.png --scene one-session`. Expected: two PNGs that look like the left and right halves of `docs/images/screens/newuxui/main-window.png` (sidebar, toolbar, table header, one row). Attach both to the task report.

- [ ] **Step 3: Manual check with the real keychain and real AWS (owner).** With a real IAM user key pair: `cargo run -p leapp-app`; add the session; start it; run `aws sts get-caller-identity --profile default` → shows the IAM user's ARN; stop; the `[default]` section is gone (or the user's original is back). Quit with the window close button → `~/.aws/credentials` has no section from Leapp RS. Record the result in the task report. If the keychain asks for a password at every build, follow spec 8.1: sign the debug binary with an Apple Development certificate (`codesign -s "Apple Development: <name>" target/debug/leapp-app`) and add the exact command to `leapp-rs/CLAUDE.md`.

- [ ] **Step 4: Full check.** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` → all pass.

- [ ] **Step 5: Commit.** `git commit -m "feat(leapp-app): real app entry point with keychain and cleanup on quit"`

---

## Spec coverage (F0 acceptance list → task)

| F0 acceptance item (spec 16) | Task |
|---|---|
| Start writes `[default]` with 4 keys in order; `DurationSeconds=36000`; stop removes; other content byte-identical | 3 |
| User-written `[default]`: prompt; no → fails, unchanged; yes → replaced; stop → original back; backup gone | 4 (core), 7 (UI) |
| Crash simulation then `startup()` cleans | 4 |
| Second session on the same profile stops the first | 3 |
| `workspace.json` contains no key or secret | 2, 3, 4 |
| App E2E: create via dialog, start, check file and status, stop | 6, 7 |
| Main window with sidebar and table (Session, Provider, Named Profile, Region, Status) | 5 |
| Cargo workspace, Rune from `examples/hello`, pinned rev, `CLAUDE.md` | 1, 5, 8 |
