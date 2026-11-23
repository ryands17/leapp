# Leapp RS: design

- Date: 2026-11-23
- Status: draft, for owner review
- Language rule: this document uses ASD-STE100 Simplified Technical English where possible.
- Audience: the owner, and builders (people or subagents) who build one feature at a time.
  A builder must be able to build a feature from this document and the referenced Leapp source,
  with no other context.

---

## 1. Intent

Leapp (this repository, TypeScript and Electron) is not maintained. The owner wants a fast AWS
credentials app that one person can maintain.

We write a new desktop app in Rust. The UI framework is Rune (https://github.com/anomalyco/rune).
The app keeps the Leapp functions for AWS and the look of the current Leapp UI (sidebar plus
session table). It adds one new function: region control for SSO integrations.

The working name is **Leapp RS**. The binary name is `leapp-rs`.

## 2. Goals

1. The app starts fast and responds fast. Target: window visible in less than 1 second on an
   Apple Silicon Mac; a click on Start gives visible feedback (status "pending") in less than
   100 ms.
2. The code is small and simple enough for one person to maintain.
3. The app does the same AWS work as Leapp. It writes temporary credentials to
   `~/.aws/credentials`, so the AWS CLI, AWS SDKs and Terraform work with no change.
4. The UI looks similar to the current Leapp UI. It does not have to be pixel-exact.
5. The app does not damage AWS profiles that the user wrote by hand (see section 9.3).

## 3. Non-goals (not in this project)

- Azure, LocalStack and other cloud providers.
- Import of old Leapp data (`~/.Leapp/Leapp-lock.json`). The owner has no old data.
- Leapp Pro and Team functions: team workspaces, remote sync, login, plans, Multi-Console browser
  extension, password lock, Touch ID lock.
- The plugin system, deep links, analytics, the in-app update check of Leapp.
- The Leapp CLI (`packages/cli`).

## 4. Later (record for future versions; do not build now)

| Item | Notes |
|---|---|
| SAML federated roles (IAM Role Federated) | Needs a browser window that can read the SAML response. Rune has no web view. Recommended: an embedded native web view window (for example the `wry` crate, WebKit on macOS). Other way: system browser plus a `localhost` listener; many identity providers do not allow this. Leapp source: `packages/core/src/services/session/aws/aws-iam-role-federated-service.ts`, `aws-saml-assertion-extraction-service.ts`. The data model must allow a 4th session type later (section 8). |
| Credential process method | Leapp can write `credential_process = leapp session generate <id>` to `~/.aws/config` (`aws-session-service.ts:94`). Needs a CLI binary. |
| CLI | A command-line binary on top of `leapp-core`. |
| Windows and Linux | Rune supports them. Keychain and terminal code is macOS only in v1. |
| Region in `~/.aws/config` | Leapp writes `region` only in the `~/.aws/credentials` section. Most AWS tools read it from there; some SDKs read `region` only from `~/.aws/config`. An optional setting can write it to `~/.aws/config` too. |

## 5. Anti-goals (failure, even if it works)

1. The app's own files (`workspace.json`, logs, caches) contain secrets in plain text.
   (Temporary credentials in `~/.aws/credentials` are allowed: that is the purpose of the app.
   Long-lived IAM access keys never go to `~/.aws/credentials`.)
2. The app changes or deletes a profile in `~/.aws/credentials` that the user wrote by hand,
   without asking, or does not put it back.
3. The AWS logic depends on the UI, so it cannot be tested without the UI.
4. A test touches the real `~/.aws`, the real keychain, or real AWS.

## 6. Words (ubiquitous language)

Use these words in code names, UI labels and documents.

| Word | Meaning |
|---|---|
| Session | One way to get AWS credentials for one account and role. Types: IAM User, IAM Role Chained, SSO Role. |
| Session status | `Inactive`, `Pending` (start or refresh in progress), `Active`. |
| Active session | A session whose temporary credentials are now in `~/.aws/credentials`. |
| Named profile | A section name in `~/.aws/credentials`, for example `default`. Each session has one named profile. |
| Managed profile | A named profile section that the app wrote and must remove or put back later. |
| Session region | The region the app writes in the session's profile section, and uses for STS calls. |
| Default region | Setting. Region for new sessions. Default `us-east-1`. |
| Integration | An AWS SSO (IAM Identity Center) connection. It creates SSO Role sessions by sync. |
| Portal URL | The AWS access portal start URL, for example `https://my-org.awsapps.com/start`. |
| Portal region | The region of the IAM Identity Center instance. Used for SSO login and SSO API calls only. |
| Integration session region | Optional region of an integration that its sessions get (feature F5). |
| Sync | Read accounts and roles from SSO, and create or remove SSO Role sessions. |
| Parent session | The session whose credentials an IAM Role Chained session uses to call `AssumeRole`. |
| Refresh | Get new temporary credentials for an active session and write them again. Leapp calls it "rotate". |
| Pin | Mark a session so it shows in the "Pinned" view. |
| Segment | A saved search and filter, shown in the sidebar. |

## 7. Approaches

| | Approach | Trade-off | Decision |
|---|---|---|---|
| A | Cargo workspace with two crates: `leapp-core` (no UI) and `leapp-app` (Rune) | A little more setup. Core is easy to test; a CLI can use it later. | **Chosen by the owner** |
| B | One crate | Fastest start. AWS logic and UI mix; harder to test. | Not chosen |
| C | Like A, but raw HTTP calls instead of the AWS SDK | Smaller binary. We must maintain request signing ourselves. | Not chosen |

## 8. Decisions

### 8.1 Made with the owner

- Rust, Rune, Cargo workspace (approach A).
- AWS only. Session types in v1: IAM User, IAM Role Chained, SSO Role. SAML is later.
- Credentials work the same as Leapp (section 10).
- The app keeps profiles that the user wrote by hand (section 9.3). This is a change from Leapp,
  which moves the old credentials file to `credentials.leapp.bkp` on first start.
- UI: the new Leapp UI (sidebar plus session table), similar but not pixel-exact.
- New feature: region control for SSO integrations (F5). "Set region for all sessions" writes
  the region to all sessions of that integration, also the ones changed by hand. After that, one
  session can be changed again.
- The work is divided into features. Each feature is built, tested and shown to the owner before
  the next one starts.
- Packaging and release (signing, notarization, `.dmg`, auto-update) is the last feature (F9).
  During development, local builds are signed with an Apple Development certificate, so the
  keychain does not ask for a password after each build.

### 8.2 Made by the designer (the owner can change them)

| Decision | Reason |
|---|---|
| Code location: new folder `leapp-rs/` at the repo root, on branch `leapp-rs`. TypeScript code stays until the Rust app is ready. | Keeps the reference code next to the new code. |
| Platform: macOS first (Apple Silicon and Intel). | The owner uses macOS. |
| AWS calls: official AWS SDK for Rust (`aws-config`, `aws-sdk-sts`, `aws-sdk-sso`, `aws-sdk-ssooidc`, `aws-sdk-ssm`, `aws-sdk-ec2`). | Signing, retries and endpoints are maintained by AWS. |
| Async runtime: `tokio` in `leapp-core`. The UI calls core work off the UI thread and gets results back as messages. | The AWS SDK needs tokio. |
| Secrets: macOS Keychain through the `keyring` crate (or `security-framework`). Service name `leapp-rs`. | Same as Leapp, but a different service name, so it never mixes with Leapp data. |
| Non-secret data in `~/.leapp-rs/workspace.json`, not encrypted, with a `version` field. Written atomically (write temp file, then rename). | No secrets in it, so no encryption is needed. Atomic write prevents a broken file after a crash. |
| Same timings as Leapp (section 10.4). | Same behavior. |
| The app stops all sessions when it starts and when it quits (same as Leapp). | No old credentials stay in the file after a crash. |
| If the user closes the window, the app keeps running in the menu bar. "Quit" in the menu bar menu or Cmd+Q quits. | Refresh only works while the app runs. |
| SSO login uses the system browser (device-code flow). The app shows the user code. | Rune has no web view. Leapp also has this mode ("In-browser"). |
| Leapp's "In-app" SSO browser option is not built. | Needs a web view. |
| Logs: `~/.leapp-rs/logs/leapp-rs.log`, rotating, no secrets. Use `tracing`. | Needed for support and debugging. |

## 9. Architecture

### 9.1 Layout

```
leapp-rs/
  Cargo.toml                 workspace; members = ["crates/*"]
  rust-toolchain.toml        pin a stable toolchain
  CLAUDE.md                  commands, rules and gotchas for this Rust project
  crates/
    leapp-core/              library, no UI, no Rune dependency
      src/
        lib.rs               public API: the `Leapp` facade (section 9.2)
        model.rs             Workspace, Session, Integration, NamedProfile, Settings, Segment
        store.rs             load and save workspace.json
        secrets.rs           SecretStore trait; KeychainStore; MemoryStore (tests)
        aws_files.rs         read and write ~/.aws/credentials (section 9.3)
        sessions/            iam_user.rs, iam_role_chained.rs, sso_role.rs, mod.rs (start/stop/refresh)
        sso/                 oidc.rs (device-code login), portal.rs (list accounts/roles, role credentials), sync.rs
        refresh.rs           the refresh loop (section 10.4)
        web_console.rs
        ssm.rs
        terminal.rs          open Terminal / iTerm2 / Warp with a command
        regions.rs           the list of AWS regions
        error.rs             one error enum (thiserror)
      tests/                 integration tests through the public API
    leapp-app/               Rune binary, started from Rune examples/hello
      src/
        main.rs
        state.rs             UI state (copy of core data plus UI-only fields)
        views/               sidebar.rs, table.rs, toolbar.rs, dialogs/*.rs, settings.rs
        bridge.rs            send commands to core, receive events
      tests/                 headless E2E tests
```

### 9.2 Core API (shape, not exact signatures)

`leapp-core` exposes one facade, `Leapp`, built from a `Config`:

```rust
pub struct Config {
    pub home_dir: PathBuf,              // tests pass a temp dir; app passes the real home
    pub data_dir: PathBuf,              // default: home_dir/.leapp-rs
    pub secrets: Arc<dyn SecretStore>,  // KeychainStore in the app, MemoryStore in tests
    pub endpoints: Endpoints,           // AWS endpoint overrides for tests (None = real AWS)
    pub clock: Arc<dyn Clock>,          // tests can move time forward
    pub prompts: Arc<dyn Prompter>,     // asks the UI for an MFA code, profile-overwrite OK, SSO device code display
    pub opener: Arc<dyn Opener>,        // opens URLs and terminals; tests record calls
}
```

Functions (all `async` where they call AWS):

- Workspace: `workspace() -> Workspace` (snapshot), `subscribe() -> Receiver<Event>`.
- Sessions: `create_iam_user(req)`, `create_iam_role_chained(req)`, `update_session(id, req)`,
  `delete_session(id)`, `start(id)`, `stop(id)`, `stop_all()`, `change_region(id, region)`,
  `change_profile(id, profile_id)`, `pin(id)`, `unpin(id)`, `credentials(id)` (for web console and SSM).
- Profiles: `create_profile(name)`, `rename_profile(id, name)`, `delete_profile(id)`.
- Integrations: `create_integration(req)`, `update_integration(id, req)`, `delete_integration(id)`,
  `login_and_sync(id)`, `logout(id)`, `set_integration_session_region(id, Option<region>)`.
- Settings: `update_settings(settings)`.
- Segments: `save_segment(name, filter)`, `delete_segment(name)`.
- Extras: `web_console_url(id)`, `open_web_console(id)`, `ssm_instances(id, region)`, `start_ssm(id, instance_id, region)`.
- Lifecycle: `startup()` (stop all, clean managed profiles, start refresh loop), `shutdown()`.

`Event` tells the UI what changed: `SessionsChanged`, `SessionStatus{id, status}`,
`IntegrationsChanged`, `SyncProgress{id, done, total}`, `Error{session_id: Option, message}`.

Rules:
- Every change goes through the facade. The facade saves `workspace.json` after each change.
- The UI never reads or writes files, the keychain or AWS directly.

### 9.3 AWS credentials file rules (`aws_files.rs`)

These rules protect the user's own profiles (owner decision).

1. The app writes only `~/.aws/credentials`. In v1 it never writes `~/.aws/config`.
2. If `~/.aws` or the file does not exist, create them. File mode `0600`, folder mode `0700`.
3. Edit the file as text, section by section. Keep all other sections, comments, blank lines and
   key order byte for byte. Do not parse and write the whole file with a generic INI library that
   loses comments. Write atomically (temp file in `~/.aws`, then rename), and keep mode `0600`.
4. The app keeps a list of managed profiles in `workspace.json`:
   `managed_profiles: { "<profile name>": { session_id, had_original: bool } }`.
5. **Start a session on profile P:**
   - If P is managed by another active session of the app: stop that session first (one active
     session per profile, same as Leapp).
   - Else, if a section `[P]` exists in the file and P is not managed: the user wrote it. Ask the
     user through `Prompter`: "Profile P exists in ~/.aws/credentials and was not written by
     Leapp RS. Replace it while the session is active? Leapp RS puts it back when the session
     stops." If the user says no, the start fails with a clear error and status goes back to
     `Inactive`. If yes: save the full original section text in the keychain
     (key `aws-profile-backup-<P>`; it can contain the user's own secret keys), set
     `had_original = true`.
   - Write the section `[P]` with exactly these keys, in this order:
     `aws_access_key_id`, `aws_secret_access_key`, `aws_session_token`, `region`.
     If the section exists, replace only that section, at the same place in the file.
     If not, add it at the end.
6. **Stop a session on profile P:** if `had_original`, write the saved original section back and
   delete the keychain backup. Else remove the section `[P]`. Remove P from `managed_profiles`.
7. **Startup and quit:** for each entry in `managed_profiles`, do step 6. This also cleans up
   after a crash, because the list is in `workspace.json`.
8. Rename of a named profile in the app while a session on it is active: stop, rename, start
   (same as Leapp, `named-profiles-service.ts:63`).

### 9.4 Keychain keys (service `leapp-rs`)

| Key | Value |
|---|---|
| `iam-user-<session_id>-access-key-id` | IAM user access key ID |
| `iam-user-<session_id>-secret-access-key` | IAM user secret access key |
| `iam-user-<session_id>-session-token` | JSON: cached `GetSessionToken` credentials |
| `sso-<integration_id>-access-token` | SSO access token |
| `aws-profile-backup-<profile name>` | Original text of a user profile section (9.3) |
| `proxy-password` | Proxy password (F8) |

`delete_session` and `delete_integration` delete their keys.

## 10. Behavior shared by all session types

### 10.1 Start (Leapp: `aws-session-service.ts:14`)

1. If another session with the same profile is `Pending`: fail with "Another session with the
   same named profile is starting".
2. Stop all active sessions with the same profile.
3. Set status `Pending`.
4. Get credentials (type-specific, section 11).
5. Write the profile section (9.3).
6. Set status `Active`, set `start_time = now`.
7. On any error: set status `Inactive`, send `Event::Error` with the AWS error message, log it.

### 10.2 Stop

If the session is `Inactive`, do nothing. Else remove or restore the profile section (9.3), set
status `Inactive`, clear `start_time`.

### 10.3 Delete (Leapp: `aws-session-service.ts:60`)

Stop the session if active. Delete all IAM Role Chained sessions whose parent is this session
(stop them first if active). Delete the session and its keychain keys. Remove it from pins.

### 10.4 Refresh loop and timings (Leapp: `rotation-service.ts`, `app.component.ts:151`, `constants.ts`)

| Constant | Value | Use |
|---|---|---|
| `REFRESH_TICK` | 10 s | Interval of the refresh loop. |
| `SESSION_DURATION` | 1200 s (20 min) | An active session is refreshed when `now - start_time > SESSION_DURATION`. |
| `IAM_USER_TOKEN_DURATION` | 36000 s (10 h) | `GetSessionToken` duration. |
| `ASSUME_ROLE_DURATION` | 3600 s (1 h) | `AssumeRole` duration for chained roles. |
| `WEB_CONSOLE_DURATION` default | 3600 s, setting (Leapp default UI value is 1 h; max 12 h) | Federation sign-in duration. |
| `SSO_MAX_TPS` | 5 | Max SSO `ListAccountRoles` calls per second. |
| `STS_TIMEOUT` | 10 s | Timeout for STS calls. |

Refresh = steps 3 to 6 of 10.1 for an active session (no stop of other sessions). An error in a
refresh sets status `Inactive`, removes the section and sends `Event::Error` (F8 shows a
notification).

The integration online state is checked in the same loop: an integration is "online" while
`now < access_token_expiration`.

### 10.5 STS calls

Use the session region for STS (regional endpoint), as Leapp does (`aws-core-service.ts:366`).
If the region is empty, use `us-east-1`. No retries for STS (Leapp sets `maxAttempts: 0`).

## 11. Session types

### 11.1 IAM User (Leapp: `aws-iam-user-service.ts`, `models/aws/aws-iam-user-session.ts`)

Fields: `name`, `region`, `profile_id`, `mfa_device` (optional ARN), `account` (optional, from
`GetCallerIdentity` for display). Secrets: access key ID and secret access key in keychain.

Get credentials:
1. If a cached token exists in the keychain and `session_token_expiration > now`: use it.
2. Else call `sts:GetSessionToken` with the long-lived keys, `DurationSeconds = 36000`.
   If `mfa_device` is set: ask the user for the 6-digit code through `Prompter`
   (dialog title "MFA code for <session name>"). If the user cancels: fail with
   "Missing Multi Factor Authentication code". Pass `SerialNumber` and `TokenCode`.
3. Save the new token JSON in the keychain and `session_token_expiration` in the session.
4. Trim spaces from all key values (Leapp does this).

Update: name, region, MFA device, profile. Access key and secret key change only if the user
types new values (empty field = keep the old value). After a key change, delete the cached token.

### 11.2 IAM Role Chained (Leapp: `aws-iam-role-chained-service.ts`)

Fields: `name`, `region`, `profile_id`, `role_arn`, `parent_session_id`, `role_session_name`
(optional; default `assumed-from-leapp`), `external_id` (optional; new in Leapp RS, standard
`AssumeRole` parameter).

Get credentials:
1. Get the parent session's credentials (any type; this does not start the parent and does not
   write the parent's profile).
2. Call `sts:AssumeRole` with the parent credentials: `RoleArn`, `RoleSessionName`,
   `DurationSeconds = 3600`, `ExternalId` if set.
3. Save `session_token_expiration`.

A session can not be its own parent. A chain can be more than one level (a chained session can
be the parent of another). Reject loops on create and update.

Account number for display: chars 13..25 of the role ARN (`arn:aws:iam::123456789012:role/x`).

### 11.3 SSO Role (Leapp: `aws-sso-role-service.ts`)

Fields: `name` (= account name), `region`, `profile_id`, `role_arn`, `integration_id`, `email`
(account email). SSO Role sessions are created only by sync, and can not be edited except
region, profile and pin. They can not be created by hand.

Role ARN format: Leapp uses `arn:aws:iam::<account id>/<role name>` (not a real IAM ARN). Keep
this internal format, so that the account ID is chars 13..25 and the role name is after `/`.
Store `account_id` and `role_name` as separate fields too, to avoid string parsing.

Get credentials:
1. Get the integration access token (11.4 step "token"). If none or expired: run login (11.4).
2. Call `sso:GetRoleCredentials` (`accountId`, `roleName`, `accessToken`) in the portal region.
3. If the call fails: log in again one time, then call again (Leapp does this).
4. Save `session_token_expiration` from the response.

### 11.4 SSO integration (Leapp: `integration/aws-sso-integration-service.ts`, `aws-sso-oidc.service.ts`)

Fields: `id`, `alias`, `portal_url`, `portal_region`, `session_region` (optional, F5),
`access_token_expiration` (optional). Token in keychain.

Validation: alias not empty; portal URL matches `https?://...` (Leapp regex,
`aws-sso-integration-service.ts:22`).

Login (device-code flow):
1. Follow HTTP redirects of the portal URL and use the final URL (Leapp does this, line 229).
2. `ssooidc:RegisterClient` (`clientName = "leapp-rs"`, `clientType = "public"`).
3. `ssooidc:StartDeviceAuthorization` (`clientId`, `clientSecret`, `startUrl`).
4. Open `verificationUriComplete` in the system browser. Show a dialog with the user code and a
   Cancel button.
5. Poll `ssooidc:CreateToken` (`grantType = urn:ietf:params:oauth:grant-type:device_code`) every
   `interval` seconds (default 5). `AuthorizationPendingException` = keep polling.
   `SlowDownException` = add 5 s to the interval. Any other error or `expiresIn` reached = fail
   with "AWS SSO login timed out. Log in again." Cancel = fail with "AWS SSO login cancelled".
6. Save the token in the keychain, `access_token_expiration = now + expiresIn`.
7. Only one login runs at a time for each integration. A second request waits for the first
   result (Leapp: `loginMutex`).

Sync (Leapp lines 80 to 125, 281 to 340):
1. Get a token (log in if needed).
2. `sso:ListAccounts` (page size 30, follow `nextToken`).
3. For each account: `sso:ListAccountRoles` (page size 30, follow `nextToken`), max 5 calls per
   second, with retry and backoff on throttling. Send `SyncProgress` after each account.
4. For each (account, role), build a candidate: `name = accountName`, `email = emailAddress`,
   `role_arn`, `integration_id`. Region and profile:
   - If a session with the same `email` and `role_arn` exists in this integration: keep its
     region and profile.
   - Else region = integration `session_region` if set (F5), else the default region.
     Profile = `default`.
5. Match key between old and new: (`name`, `role_arn`, `email`). Old sessions with no match: delete
   (10.3). New candidates with no match: create.
6. Show the result: "Sync done: X added, Y removed".

Logout: call `sso:Logout` with the token (ignore "Session token not found or invalid"), delete the
token, clear `access_token_expiration`, stop all sessions of the integration.

Delete integration: logout, then delete all its sessions (10.3), then delete the integration.

## 12. Data model (`workspace.json`)

```jsonc
{
  "version": 1,
  "sessions": [
    {
      "id": "uuid",
      "name": "string",
      "region": "eu-west-1",
      "profile_id": "uuid",
      "status": "inactive",              // saved, but always reset to inactive at startup
      "start_time": null,                // RFC 3339
      "session_token_expiration": null,  // RFC 3339
      "kind": {                          // tagged enum; room for "iam_role_federated" later
        "type": "iam_user",
        "mfa_device": null,
        "account": null
      }
      // or { "type": "iam_role_chained", "role_arn", "parent_session_id", "role_session_name", "external_id" }
      // or { "type": "sso_role", "role_arn", "account_id", "role_name", "integration_id", "email" }
    }
  ],
  "profiles": [ { "id": "uuid", "name": "default" } ],   // "default" always exists, cannot be deleted
  "integrations": [
    { "id": "uuid", "alias": "my-org", "portal_url": "https://...", "portal_region": "eu-west-1",
      "session_region": null, "access_token_expiration": null }
  ],
  "managed_profiles": { "default": { "session_id": "uuid", "had_original": false } },
  "pinned": ["session uuid"],
  "segments": [ { "name": "IAM Users", "filter": { /* section 14.4 */ } } ],
  "settings": {
    "default_region": "us-east-1",
    "color_theme": "system",            // system | light | dark
    "macos_terminal": "Terminal",       // Terminal | iTerm2 | Warp
    "web_console_duration_hours": 1,    // 1..12
    "ssm_region_behaviour": "no",       // no | use_default_region
    "proxy": { "enabled": false, "protocol": "https", "url": null, "port": "8080", "username": null },
    "visible_columns": ["session", "role", "provider", "named_profile", "region"]
  }
}
```

Rules:
- If `version` is higher than the app knows, show an error and do not write the file.
  (v1 has no older format, so no migration code is necessary yet.)
- If the file is not valid JSON: rename it to `workspace.json.broken-<timestamp>`, start with an
  empty workspace, and show a warning.

## 13. Regions

Use the region list of Leapp (`aws-core-service.ts:320`, the keys of `stsEndpointsPerRegion`),
sorted. Show region codes in uppercase in the table chip (for example `EU-WEST-1`), lowercase in
dialogs and files.

## 14. UI

Reference images in this repo:
- Main window: `docs/images/screens/newuxui/main-window.png`
- Dark theme: `docs/images/screens/newuxui/leapp-dark.png`
- Settings: `docs/images/screens/newuxui/leapp-options.png`
- Create session: `docs/images/screens/create.png`, `docs/images/screens/select.png`,
  `docs/images/screens/newuxui/aws-iam-user.png`, `aws-iam-role-chained.png`, `aws-sso.png`
- Videos: `docs/videos/newuxui/*.mp4` (extract frames with `ffmpeg`)
- Leapp Angular source: `packages/desktop-app/src/app/components/` (HTML and SCSS give exact
  spacing, colors and labels).

Builders must copy labels and layout from these sources. Exact pixels are not required.

### 14.1 Main window

```
┌────────────────┬────────────────────────────────────────────────────────────────┐
│ ● ● ●      [▥] │ (+)            [≡]  [ 🔍 Search session          ]  [⟳]    [⚙] │
│                ├────────────────────────────────────────────────────────────────┤
│ ▤ All Sessions │ Session            Role         Provider      Named Profile Region [▦]│
│ 📌 Pinned      ├────────────────────────────────────────────────────────────────┤
│                │ aws prod-admin     Admin        (AWS SSO)     default    EU-WEST-1  ●│
│ Integrations ⊕ │ aws My IAM User                 (IAM User)    default    US-EAST-1  ○│
│  aws● my-org ⋯ │ aws deploy-role    deploy       (IAM Chained) ci         EU-WEST-1  ○│
│  ⊕ Add         │                                                                │
│                │                                                                │
│ Saved segments^│                                                                │
│  🏷 IAM Users  │                                                                │
├────────────────┴────────────────────────────────────────────────────────────────┤
│ status bar: "Fetched 3 of 12 accounts..." (only during sync)                    │
└─────────────────────────────────────────────────────────────────────────────────┘
```

- Sidebar can collapse ([▥]).
- Toolbar: (+) create session; [≡] filter menu; search; [⟳] sync all online integrations; [⚙] settings.
- Table rows: AWS icon, session name, role (role name for SSO and chained, empty for IAM User),
  provider chip (colors: IAM User orange, IAM Role Chained purple, AWS Single Sign-On red),
  named profile, region chip, status dot (grey = inactive, spinner = pending, orange/green = active).
- Active sessions are listed first, then the others, each group sorted by name.
- Double-click a row or press Enter: start or stop.
- Right-click a row or its ⋯ button: row menu (14.3).
- Empty state (no sessions): a centered message "No sessions yet" and buttons "Add session" and
  "Add AWS SSO integration".

### 14.2 Integration item in the sidebar

- Dot: green = online (token valid), grey = offline. Tooltip: "Expires in N hours".
- Click: shows only that integration's sessions in the table.
- ⋯ menu: **Log in and sync** (or **Sync** when online), **Log out**, **Set region for all
  sessions…** (F5), **Edit**, **Delete**.

### 14.3 Row menu

Start session / Stop session; Open web console and Copy web console URL (F7; SSO Role and
IAM Role Chained only); View SSM
sessions (F7); Change region…; Change named profile…; Pin / Unpin; Copy account number;
Copy role ARN (chained and SSO); Edit session (IAM User and chained only); Delete session.

### 14.4 Filter menu and segments (Leapp: `models/segment.ts`, `filter-menu` component)

Filter fields: search text, pinned only, provider types (IAM User, IAM Role Chained, AWS SSO),
named profiles, regions, integrations. Search matches name, role, profile, region, account
number, case-insensitive. "Save segment" stores the current filter with a name. Clicking a
segment applies its filter. Segments can be deleted.

### 14.5 Create session dialog

Step 1: choose type: "AWS IAM User", "AWS IAM Role Chained", "AWS Single Sign-On" (the last one
opens the add-integration dialog). Step 2: type form.

- IAM User: Session alias*, AWS Access Key ID*, AWS Secret Access Key* (masked, eye toggle),
  MFA Device ARN, Named profile (select or type a new name), Region (select, default from settings).
- IAM Role Chained: Session alias*, Assumer session* (select from all sessions), Role ARN*,
  Role session name, External ID, Named profile, Region.
- Validation messages appear under the field. Create is disabled until required fields are valid.

### 14.6 Add / edit integration dialog

Alias*, Portal URL*, Portal region* (label "Portal region (IAM Identity Center region)"),
Session region (label "Region for all sessions (optional)", F5).

### 14.7 Small dialogs

- Change region: one region select. Change named profile: select or type a new name.
- MFA code: 6-digit field, auto focus, Enter submits.
- SSO device code: shows user code in large text, "Open browser again", Cancel.
- Profile overwrite confirm (9.3 step 5).
- Delete confirm: "Delete session <name>? N chained sessions that use it will also be deleted."

### 14.8 Settings window (tabs like Leapp `options-dialog`)

- General: color theme; default region; macOS terminal; web console duration (hours, 1 to 12);
  SSM region behaviour.
- Profiles: list of named profiles with rename and delete; add.
- Proxy: enabled, protocol, URL, port, user, password.
- About: version, link to logs folder.

### 14.9 Menu bar (tray) menu (F8)

Shows active sessions first, then pinned sessions, each with start/stop; "Open Leapp RS"; "Quit".

## 15. Testing

Follow the owner's rules (global CLAUDE.md):
- E2E first. Run the red-green loop at E2E or integration level.
- Each test is a committed file; one command runs it again.
- If a unit must be tested alone, first write the list of failure modes and show it to the
  owner, then the tests, then the code.
- No tautological tests: each test must fail when the behavior it covers breaks.

### 15.1 Commands

- All tests: `cd leapp-rs && cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`
- Run the app: `cargo run -p leapp-app`
- Headless check: see the Rune README (`crates/rune/README.md` in the Rune repo) for render to
  PNG and element-tree dump. Put rendered PNGs in `leapp-rs/target/shots/` (not committed).

### 15.2 Core integration tests (`crates/leapp-core/tests/`)

- Each test makes a temp home dir, a `MemoryStore`, a fake `Clock`, a scripted `Prompter` and a
  recording `Opener`.
- AWS: a local fake AWS HTTP server started by the test (for example `axum` or `wiremock` on a
  random port), passed through `Config.endpoints`. It implements only the operations the app
  calls: STS `GetSessionToken`, `AssumeRole`, `GetCallerIdentity`; SSO-OIDC `RegisterClient`,
  `StartDeviceAuthorization`, `CreateToken`; SSO `ListAccounts`, `ListAccountRoles`,
  `GetRoleCredentials`, `Logout`; SSM `DescribeInstanceInformation`; EC2 `DescribeInstances`;
  the federation `getSigninToken` endpoint. The fake records each request, so tests can check
  parameters (for example `DurationSeconds`, `SerialNumber`, `ExternalId`).
  A local AWS emulator can replace parts of it if it supports the operation; the builder decides.
- Assertions check real outputs: the text of `<home>/.aws/credentials`, `workspace.json`, the
  `MemoryStore` contents, the fake server's request log, and events.

### 15.3 App E2E tests (`crates/leapp-app/tests/`)

- Run the Rune app headless with a core `Config` like 15.2.
- Drive it with scripted input (click by element key or label, type text), then check the
  element tree and the files.
- Give every interactive element a stable `key` (for example `row-<session id>`,
  `btn-create`, `dlg-iam-user-access-key`) so tests do not depend on layout.
- Visual check: render PNGs of main screens in light and dark, and compare them by eye with the
  Leapp reference images at the end of each UI feature. These PNGs are review aids, not tests.

### 15.4 Manual check with real AWS

At the end of F1, F3, F4 and F5, the owner runs the app against a real AWS account and checks
`aws sts get-caller-identity --profile <p>` works. This is a manual step, not a CI test.

## 16. Features (build order)

Each feature gets its own implementation plan in `docs/superpowers/plans/`. Build one, test it,
show it to the owner, then start the next one. Each feature lists its Leapp references, its
behavior (section numbers above) and its acceptance tests. "Done" means all listed acceptance
tests pass with `cargo test --workspace`, clippy is clean, and the owner saw a demo.

### F0. Tracer bullet: one IAM User session, end to end

Status: complete (merged to `master` on 2026-11-24). Deferred items are in
`docs/superpowers/plans/2026-11-24-leapp-rs-f0-carry-forward.md`.

Scope:
- `leapp-rs/` Cargo workspace, `rust-toolchain.toml`, `leapp-rs/CLAUDE.md` with commands.
- `leapp-app` from Rune `examples/hello` (pin a Rune `dev` rev in `Cargo.toml`).
- `leapp-core`: `Config`, store (12, sessions and profiles only), `SecretStore` (keychain and
  memory), `aws_files` (all of 9.3, including the overwrite prompt), IAM User get-credentials
  without MFA (11.1 steps 1, 2, 3, 4), start/stop (10.1, 10.2), startup and shutdown cleanup.
- Fake AWS server with `GetSessionToken`.
- UI: main window with sidebar ("All Sessions" only) and table (Session, Provider, Named Profile,
  Region, Status). Create IAM User dialog (alias, keys, profile from a select with only
  `default` plus "type new name", region). Click a row's status or double-click to start/stop.
- No refresh loop, no MFA, no edit or delete yet.

Acceptance tests:
- Core: create IAM User → start → credentials file has `[default]` with the 4 keys in order,
  values from the fake server; `DurationSeconds=36000` was sent; stop → section is gone; other
  sections and comments in the file are byte-identical before and after.
- Core: a user-written `[default]` exists → start asks the prompter; "no" → start fails, file
  unchanged; "yes" → replaced; stop → original section text back, keychain backup gone.
- Core: crash simulation: start, drop the `Leapp` without shutdown, make a new `Leapp` on the
  same dirs, call `startup()` → section removed or restored.
- Core: start a second session on the same profile → first session becomes inactive.
- Core: `workspace.json` contains no access key or secret value (search the file text).
- App E2E: create a session through the dialog, start it, check the file and the status dot,
  stop it.

### F1. IAM User complete

Scope: MFA (11.1 step 2) with the MFA dialog; token cache use; edit and delete (10.3, 11.1
update); refresh loop (10.4) for all types; row menu with Start/Stop, Edit, Delete, Copy
account number (from `GetCallerIdentity` after the first start, cached in `account`).

Acceptance tests:
- MFA: prompter returns "123456" → fake server saw `SerialNumber` and `TokenCode=123456`;
  prompter cancels → error "Missing Multi Factor Authentication code", status inactive.
- Cache: start, stop, start again before expiry → only one `GetSessionToken` call.
- Expiry: move clock past `session_token_expiration` → next start calls `GetSessionToken` again.
- Refresh: start, move clock 21 min, run one tick → new credentials in the file.
- Refresh error: fake returns an error on refresh → status inactive, section removed, error event.
- Edit with new keys → cached token deleted, next start uses new keys (check request auth key id).
- Delete → keychain keys gone, session gone, section gone if it was active.
- App E2E: MFA dialog flow; edit dialog keeps secret fields empty and keeps old keys.

### F2. Named profiles and per-session region

Scope: settings "Profiles" tab (14.8); create profile from the session dialogs; rename (9.3
step 8) and delete (sessions move to `default`, restart if active; Leapp
`named-profiles-service.ts:72`); name validation (not empty, not `default`, unique;
Leapp line 117); "Change region…" and "Change named profile…" row actions (stop, change, start
again if active; Leapp `regions-service.ts`); settings "Default region".

Acceptance tests:
- Change region of an active session → section `region` updated, session still active.
- Change profile of an active session → old section gone, new section written.
- Rename profile with an active session → section moves to the new name.
- Delete profile → its sessions now use `default`.
- Validation messages for empty, `default`, and duplicate names.
- New session dialog preselects the default region from settings.

### F3. IAM Role Chained

Scope: 11.2 completely; create and edit dialog (14.5); parent select lists all sessions except
itself and its descendants; delete cascade (10.3); Copy role ARN.

Acceptance tests:
- Parent IAM User + chained → start chained → fake saw `AssumeRole` with parent's temporary
  credentials, `RoleSessionName=assumed-from-leapp` (or the set value), `DurationSeconds=3600`,
  and `ExternalId` when set; parent's profile section was not written.
- Two-level chain works.
- Loop is rejected on edit.
- Delete parent → chained sessions deleted, and stopped first if active.
- Parent with MFA → MFA prompt appears when chained starts and parent token is expired.

### F4. AWS SSO integration

Scope: 11.3 and 11.4 completely; sidebar "Integrations" (14.2) without the F5 menu item;
add/edit/delete integration dialog (14.6 without session region); device-code dialog (14.7);
sync progress in the status bar; [⟳] toolbar button syncs all online integrations; integration
filter (click in sidebar).

Acceptance tests (fake server implements OIDC and SSO):
- Login: `CreateToken` returns pending twice, then a token → token in keychain, expiration set,
  opener got `verificationUriComplete`.
- Login cancel and login timeout give the right errors.
- Sync: 2 accounts × 2 roles with paging on both calls → 4 sessions, names = account names.
- Re-sync after a role is removed on the server → that session deleted; a kept session keeps
  its changed region and profile.
- Throttle: no more than 5 `ListAccountRoles` calls in any 1-second window (check timestamps
  in the request log).
- Start SSO session → `GetRoleCredentials` with right account and role; failure → one re-login
  and retry.
- Logout → `Logout` called, token deleted, the integration's active sessions stopped.
- Delete integration → its sessions and token are gone.
- App E2E: add integration, log in (scripted prompter), see rows appear.

### F5. Region control for SSO (new feature)

Scope:
- Integration field `session_region` (optional) in the add/edit dialog.
- Integration menu "Set region for all sessions…": dialog with one region select and the text
  "This changes the region of all N sessions of <alias>, also sessions you changed by hand."
  On OK: set `session_region`, then set the region of every session of the integration; active
  sessions are rewritten at once (no stop of other sessions, profile stays).
- After that, "Change region…" on one session works as in F2.
- Sync: new sessions get `session_region` if set, else the default region (11.4 sync step 4).
  Existing sessions keep their region.
- The integration dialog can clear `session_region` (select "None (use default region)").
  Clearing changes no session.
- Labels: "Portal region (IAM Identity Center region)" vs "Region for all sessions".

Acceptance tests:
- 3 SSO sessions, one changed by hand to `us-west-2`, one active → "Set region for all" to
  `eu-central-1` → all 3 have `eu-central-1`; the active one's section `region` is updated and
  it is still active.
- Then change one session to `eu-west-1` → only that one changes.
- Sync after "Set region for all" with one new role on the server → the new session has
  `eu-central-1`; others unchanged.
- With `session_region` empty, a new synced session gets the settings default region.
- Sessions of a second integration do not change.
- App E2E: open the integration menu, set region, check the region chips in the table.

### F6. Session table functions

Scope: search; filter menu (14.4); pin/unpin and "Pinned" view; segments (save, apply, delete);
column selector (`visible_columns`); Role column; sorting (active first, then name); empty state;
keyboard (arrow keys move selection, Enter start/stop, Cmd+F focus search).

Acceptance tests (mostly app E2E):
- Search "prod" shows only matching rows; clears with Esc.
- Filter by provider and region combine with AND across groups, OR inside a group.
- Pin → row in "Pinned" view; pins survive restart.
- Save segment → appears in sidebar after restart; applying it sets the filter.
- Hide the Role column → column gone, setting saved.

### F7. Web console and SSM

Scope:
- Web console (Leapp `web-console-service.ts`): get session credentials (no profile write), call
  `https://us-east-1.signin.aws.amazon.com/federation?Action=getSigninToken&sessionDuration=<s>&Session=<json>`,
  build the login URL (`Action=login`, `Issuer=Leapp RS`, `Destination=https://<region>.console.aws.amazon.com/console/home?region=<region>`),
  wrap it in the `oauth?Action=logout&redirect_uri=` URL as Leapp does, open it or copy it.
  GovCloud regions use the `amazonaws-us-gov.com` hosts. China regions: error "Unsupported region".
  Only SSO Role and IAM Role Chained sessions show these menu items. IAM User sessions do not,
  because AWS does not accept `GetSessionToken` credentials for console sign-in (Leapp:
  `contextual-menu.component.html:47`).
- SSM (Leapp `ssm-service.ts`): dialog lists online SSM instances (`DescribeInstanceInformation`
  with `PingStatus == Online`, page size 50) with names from EC2 `Name` tags
  (`DescribeInstances`, page size 100). Region select in the dialog; its initial value follows
  "SSM region behaviour" (session region, or default region). Start opens the chosen terminal
  with `aws ssm start-session --region <r> --target <id>` and the three `AWS_*` env vars. Do not
  write credentials to a file that stays: if a temp file is needed for Terminal/iTerm2
  AppleScript, use mode 0600 and delete it after the terminal starts.
- Settings: macOS terminal, web console duration, SSM region behaviour.

Acceptance tests:
- Web console URL built from the fake federation endpoint has the right `Destination` and
  `SigninToken`; duration in the request equals the setting in seconds.
- `cn-north-1` → "Unsupported region".
- SSM list: 3 instances, 1 offline, 1 with a Name tag → 2 rows, one shows its Name.
- Start SSM → opener got the right command and env; no credentials file left in the home dir.

### F8. Settings, notifications, menu bar, finish

Scope: settings window (14.8) complete; color theme (system/light/dark, follows the system when
"system"); proxy (all AWS clients and the federation call use it; password in keychain);
macOS notifications on refresh failure and on SSO token expiry; menu bar menu (14.9; Rune has
no tray API, use a tray crate on the main thread through Rune's `AppHandle`); window close keeps
the app in the menu bar; Cmd+Q quits and stops all sessions; logs (8.2).

Acceptance tests:
- Theme setting persists and switches the kit colors (element tree shows the theme).
- Proxy setting is passed to AWS clients (fake server sees requests through a test proxy).
- Refresh failure → notification sent (recording notifier).
- Quit → all managed profiles cleaned (core test on `shutdown()`).

### F9. Packaging and release (last)

Scope: app bundle (`Leapp RS.app`) with icon and `Info.plist`; signing with Developer ID;
notarization; `.dmg`; GitHub Actions workflow on tag (the repo already has `APPLE_TEAM_ID` in
CI); auto-update (for example Sparkle through a crate, or a simple "new version" check against
GitHub releases; builder proposes, owner decides). Until F9, local builds are signed with an
Apple Development certificate (document the command in `leapp-rs/CLAUDE.md`).

## 17. Left to the builder

- Exact Rune and `rune-kit` components, spacing and colors (copy the Leapp look from 14).
- Crates for keychain, tray, notifications, fake HTTP server, UUIDs, time.
- Internal module layout of `leapp-core` beyond 9.1.
- How the UI thread and the tokio runtime talk (channels, Rune `Tx`), as long as the UI never
  blocks on AWS calls.
- Error message wording, except the messages quoted in this document.

## 18. Risks

| Risk | Mitigation |
|---|---|
| Rune is pre-release; APIs change; the team does not take pull requests. | Pin a `dev` rev. Keep Rune code in `leapp-app` only. Report Rune bugs as issues. |
| Rune may not have a table component. | Build the table from rows of fixed-width columns. Check `rune-kit` first. |
| Keychain prompts during development. | Sign local builds (8.1). Tests use `MemoryStore`. |
| AWS SDK for Rust adds build time and binary size. | Enable only needed features; use `rustls`; accept it (approach C was rejected). |
| Some tools do not read `region` from `~/.aws/credentials`. | Same as Leapp in v1; "Region in ~/.aws/config" is a later option (4). |
