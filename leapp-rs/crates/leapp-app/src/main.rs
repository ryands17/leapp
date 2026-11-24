//! The Leapp RS desktop app.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use leapp_app::{Bridge, Msg, UiPrompter};
use leapp_core::{Config, KeychainStore, Leapp, MemoryStore, REFRESH_TICK, SecretStore};
use tempfile::TempDir;

/// Where the app keeps its files and secrets for this run.
struct Env {
    home: PathBuf,
    secrets: Arc<dyn SecretStore>,
    /// Holds the temporary home of a headless run. Drop it only when the run ends.
    _temp: Option<TempDir>,
}

impl Env {
    /// A real run uses the home folder and the keychain. A headless run (`--snap`, `--tree`,
    /// `--script`, `--list-scenes`, ...) uses a temporary home and a `MemoryStore`, so it never
    /// touches `~/.aws` or the keychain.
    fn new(headless: bool) -> Result<Env, String> {
        if headless {
            let temp = tempfile::tempdir().map_err(|e| format!("no temporary home: {e}"))?;
            Ok(Env {
                home: temp.path().to_path_buf(),
                secrets: Arc::new(MemoryStore::default()),
                _temp: Some(temp),
            })
        } else {
            let home = std::env::home_dir().ok_or("no home folder")?;
            Ok(Env {
                home,
                secrets: Arc::new(KeychainStore::new("leapp-rs")),
                _temp: None,
            })
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("leapp-app: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    let env = Env::new(rune::is_headless())?;
    let bridge = Bridge::new();
    let config = Config::new(
        env.home.clone(),
        env.secrets.clone(),
        Arc::new(UiPrompter::new(bridge.clone())),
    );
    let leapp = Arc::new(Leapp::open(config).map_err(|e| format!("cannot open the data: {e}"))?);

    // A failed startup does not stop the app: the UI shows the error in its toast.
    if let Err(e) = rt.block_on(leapp.startup()) {
        eprintln!("leapp-app: startup failed: {e}");
        bridge.send(Msg::Failed(e.to_string()));
    }

    let refresh_loop =
        leapp_app::spawn_refresh_loop(leapp.clone(), rt.handle(), bridge.clone(), REFRESH_TICK);
    if !leapp_app::install(
        leapp,
        rt.handle().clone(),
        bridge,
        refresh_loop.abort_handle(),
    ) {
        return Err("bug: leapp_app::install ran two times".into());
    }
    // Closing the window quits. `on_close` also runs for Cmd+Q and the Dock quit.
    rune::App::new(leapp_app::spec())
        .on_close(leapp_app::on_close)
        .run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_run_uses_a_temp_home_and_memory_store() {
        let real = std::env::home_dir().unwrap();
        let env = Env::new(true).unwrap();
        assert_ne!(env.home, real);
        assert!(env.home.is_dir());
        // A `MemoryStore` accepts a write without a keychain.
        env.secrets.set("k", "v").unwrap();
        assert_eq!(env.secrets.get("k").unwrap().as_deref(), Some("v"));
    }
}
