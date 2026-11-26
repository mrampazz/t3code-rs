//! The app's own server, like the desktop app's (`apps/desktop/src/backend`): reuse the server
//! that already owns the T3 home (the desktop app, `t3 serve`, or the background service), else
//! start `t3 serve` as a child that quits with the app; then issue a bearer session with the CLI.
//! `T3_GPUI_HOME` picks the home (default `~/.t3`), `T3_GPUI_CLI` the command (default `t3`),
//! and `T3_GPUI_NO_LOCAL_SERVER=1` turns this off.
//! shortcut: no crash restarts or version checks against the running server (desktop has both).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use crate::auth::Session;

/// The server this app started, stopped on quit.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

const START_TIMEOUT: Duration = Duration::from_secs(90);

pub fn enabled() -> bool {
    std::env::var_os("T3_GPUI_NO_LOCAL_SERVER").is_none() && std::env::var_os("T3_SERVER").is_none()
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("T3_GPUI_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".t3")))
}

fn cli() -> Vec<String> {
    std::env::var("T3_GPUI_CLI")
        .ok()
        .map(|c| c.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| vec!["t3".into()])
}

fn command(args: &[&str]) -> Command {
    let cli = cli();
    let mut cmd = Command::new(&cli[0]);
    cmd.args(&cli[1..]).args(args);
    cmd
}

/// The origin a running server recorded for this home (`userdata/server-runtime.json`).
pub fn recorded_origin(home: &Path) -> Option<String> {
    let bytes = std::fs::read(home.join("userdata/server-runtime.json")).ok()?;
    let state: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    state["origin"]
        .as_str()
        .map(|o| o.trim_end_matches('/').to_owned())
}

/// The recorded server, if it answers as this home's environment: a killed server leaves its
/// record behind, and another process may own its port by now.
async fn live_origin(home: &Path) -> Option<String> {
    let origin = recorded_origin(home)?;
    let expected = std::fs::read_to_string(home.join("userdata/environment-id")).ok()?;
    let descriptor: serde_json::Value = reqwest::Client::new()
        .get(format!("{origin}/.well-known/t3/environment"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    (descriptor["environmentId"].as_str() == Some(expected.trim())).then_some(origin)
}

/// The local server's URL, starting one when none owns `home`.
async fn ensure_running(home: &Path) -> Result<String> {
    if let Some(origin) = live_origin(home).await {
        return Ok(origin);
    }
    let log = crate::auth::config_dir()
        .map(|d| d.join("server.log"))
        .context("no config directory")?;
    std::fs::create_dir_all(log.parent().unwrap())?;
    let out = std::fs::File::create(&log)?;
    let child = command(&[
        "serve",
        "--base-dir",
        &home.to_string_lossy(),
        "--no-browser",
    ])
    .stdin(Stdio::null())
    .stdout(out.try_clone()?)
    .stderr(out)
    .spawn()
    .with_context(|| format!("could not start `{}`", cli().join(" ")))?;
    *CHILD.lock().unwrap() = Some(child);
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if let Some(origin) = live_origin(home).await {
            return Ok(origin);
        }
        let exited = CHILD
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten());
        if let Some(status) = exited {
            let tail = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: Vec<&str> = tail.lines().rev().take(6).collect();
            bail!(
                "the local server exited ({status}): {}",
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            );
        }
        if tokio::time::Instant::now() > deadline {
            bail!(
                "the local server did not start in time (log: {})",
                log.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// A session for the local server: a saved one for its URL, else one issued by the CLI.
pub async fn session(saved: &[Session]) -> Result<Session> {
    let home = home().context("no home directory")?;
    let origin = ensure_running(&home).await?;
    if let Some(s) = saved
        .iter()
        .find(|s| s.base_url.trim_end_matches('/') == origin)
    {
        return Ok(s.clone());
    }
    let home_arg = home.to_string_lossy().into_owned();
    let output = tokio::task::spawn_blocking(move || {
        command(&[
            "auth",
            "session",
            "issue",
            "--base-dir",
            &home_arg,
            "--token-only",
            "--ttl",
            "30d",
            "--label",
            "T3 Code native",
        ])
        .stdin(Stdio::null())
        .output()
    })
    .await??;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || token.is_empty() {
        bail!(
            "could not issue a local session: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(Session {
        base_url: origin,
        access_token: token,
        token_type: crate::auth::bearer(),
        expires_at: None,
    })
}

/// Stops the server this app started; one it found running keeps running.
pub fn stop() {
    let Some(mut child) = CHILD.lock().unwrap().take() else {
        return;
    };
    // SIGTERM lets the server release its home and clear its runtime record.
    let _ = Command::new("kill").arg(child.id().to_string()).status();
    for _ in 0..40 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_recorded_origin() {
        let dir = std::env::temp_dir().join(format!("t3-gpui-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("userdata")).unwrap();
        assert_eq!(super::recorded_origin(&dir), None);
        std::fs::write(
            dir.join("userdata/server-runtime.json"),
            r#"{"version":1,"pid":1,"port":3773,"origin":"http://127.0.0.1:3773/","startedAt":"x"}"#,
        )
        .unwrap();
        assert_eq!(
            super::recorded_origin(&dir).as_deref(),
            Some("http://127.0.0.1:3773")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
