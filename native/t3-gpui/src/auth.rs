//! Pairing and socket auth, matching `packages/contracts/src/environmentHttp.ts`.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub base_url: String,
    pub access_token: String,
}

/// `http://host:port/pair#token=XYZ` (see `buildPairingUrl` in apps/server/src/startupAccess.ts).
pub fn parse_pairing_url(input: &str) -> Result<(String, String)> {
    let url = Url::parse(input.trim()).context("not a URL")?;
    let token = url
        .fragment()
        .and_then(|f| url::form_urlencoded::parse(f.as_bytes()).find(|(k, _)| k == "token"))
        .or_else(|| url.query_pairs().find(|(k, _)| k == "token"))
        .map(|(_, v)| v.into_owned())
        .context("pairing URL has no token")?;
    Ok((url.origin().ascii_serialization(), token))
}

pub async fn pair(pairing_url: &str) -> Result<Session> {
    let (base_url, credential) = parse_pairing_url(pairing_url)?;
    #[derive(Deserialize)]
    struct TokenResult {
        access_token: String,
    }
    let res = reqwest::Client::new()
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
            ("subject_token", credential.as_str()),
            ("subject_token_type", "urn:t3:params:oauth:token-type:environment-bootstrap"),
            ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("client_label", "T3 Code (Rust)"),
            ("client_device_type", "desktop"),
        ])
        .send()
        .await?;
    if !res.status().is_success() {
        bail!("pairing failed ({}): {}", res.status(), res.text().await.unwrap_or_default());
    }
    let token: TokenResult = res.json().await?;
    let session = Session { base_url, access_token: token.access_token };
    save(&session);
    Ok(session)
}

/// The server refused the saved session; retrying cannot help, the user must pair again.
#[derive(Debug)]
pub struct Rejected(pub u16);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The server rejected this session ({}). Pair again.", self.0)
    }
}

impl std::error::Error for Rejected {}

pub async fn socket_url(session: &Session) -> Result<String> {
    #[derive(Deserialize)]
    struct Ticket {
        ticket: String,
    }
    let res = reqwest::Client::new()
        .post(format!("{}/api/auth/websocket-ticket/rpc", session.base_url))
        .bearer_auth(&session.access_token)
        .send()
        .await?;
    if matches!(res.status().as_u16(), 401 | 403) {
        return Err(Rejected(res.status().as_u16()).into());
    }
    if !res.status().is_success() {
        bail!("server returned {}", res.status());
    }
    let ticket: Ticket = res.json().await?;
    let mut url = Url::parse(&session.base_url)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" }).ok();
    url.set_path("/ws");
    url.query_pairs_mut()
        .append_pair("wsTicket", &ticket.ticket)
        .append_pair("orchestrationProtocol", "2")
        .append_pair("frameParts", "1");
    Ok(url.into())
}

fn session_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/t3-gpui/session.json"))
}

pub fn load() -> Option<Session> {
    if let (Ok(base_url), Ok(access_token)) = (std::env::var("T3_SERVER"), std::env::var("T3_TOKEN")) {
        return Some(Session { base_url, access_token });
    }
    serde_json::from_slice(&std::fs::read(session_path()?).ok()?).ok()
}

pub fn save(session: &Session) {
    let Some(path) = session_path() else { return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(&path, serde_json::to_vec(session).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

pub fn forget() {
    if let Some(path) = session_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_pairing_url() {
        let (base, token) = super::parse_pairing_url("http://192.168.1.4:3773/pair#token=abc%2B1").unwrap();
        assert_eq!(base, "http://192.168.1.4:3773");
        assert_eq!(token, "abc+1");
        assert!(super::parse_pairing_url("http://x/pair").is_err());
    }
}
