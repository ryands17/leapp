mod common;

use common::Harness;

/// A `workspace.json` as the F0 build wrote it: RFC 3339 UTC with `Z` and only the fractional
/// digits the value needs (nine in `start_time`, one in `session_token_expiration`).
const F0_WORKSPACE: &str = r#"{
  "version": 1,
  "sessions": [
    {
      "id": "5f1c1a52-6a52-4a0e-9d54-0f2f8d3f7a11",
      "name": "dev",
      "region": "eu-west-1",
      "profile_id": "0b6f0f0e-2f3c-4f0e-8d57-3f3a7c1b9a22",
      "status": "active",
      "start_time": "2026-11-24T10:00:00.123456789Z",
      "session_token_expiration": "2026-11-24T22:00:00.5Z",
      "kind": { "type": "iam_user", "mfa_device": null, "account": null }
    }
  ],
  "profiles": [
    { "id": "0b6f0f0e-2f3c-4f0e-8d57-3f3a7c1b9a22", "name": "default" }
  ]
}"#;

#[tokio::test]
async fn an_f0_workspace_file_still_loads() {
    let h = Harness::new().await;
    let path = h.workspace_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, F0_WORKSPACE).unwrap();

    let start = "2026-11-24T10:00:00.123456789Z".parse().unwrap();
    let expiry = "2026-11-24T22:00:00.5Z".parse().unwrap();

    let h = h.reopen();
    let ws = h.leapp.workspace();
    assert_eq!(ws.sessions.len(), 1, "the file must load, not move aside");
    assert_eq!(ws.sessions[0].start_time, Some(start));
    assert_eq!(ws.sessions[0].session_token_expiration, Some(expiry));
    // The F0 and F1 files have no `settings`.
    assert_eq!(ws.settings.default_region, "us-east-1");

    // A save writes the file again. The instants must survive it.
    h.leapp
        .create_iam_user(h.iam_user("other", "default"))
        .unwrap();
    let h = h.reopen();
    let ws = h.leapp.workspace();
    let dev = ws.sessions.iter().find(|s| s.name == "dev").unwrap();
    assert_eq!(dev.start_time, Some(start));
    assert_eq!(dev.session_token_expiration, Some(expiry));
}

#[tokio::test]
async fn the_broken_file_stamp_keeps_its_format() {
    let h = Harness::new().await;
    let path = h.workspace_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();

    let _h = h.reopen();
    let name = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("workspace.json.broken-"))
        .expect("the bad file is moved aside");
    let stamp = name.strip_prefix("workspace.json.broken-").unwrap();
    // Shape: 20261124T100000.123Z
    let shape: String = stamp
        .chars()
        .map(|c| if c.is_ascii_digit() { 'd' } else { c })
        .collect();
    assert_eq!(shape, "ddddddddTdddddd.dddZ", "{stamp}");
}
