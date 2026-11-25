//! Pairing and socket auth, matching `packages/contracts/src/environmentHttp.ts`.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub base_url: String,
    pub access_token: String,
    /// "Bearer", or "DPoP" for a session bound to this device's key (renewable).
    #[serde(default = "bearer")]
    pub token_type: String,
    /// Unix ms when the access token expires; renewable DPoP sessions renew before it.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// `Rejected` with the server's `WWW-Authenticate` reason (DPoP failures name their cause).
fn rejected(res: reqwest::Response) -> anyhow::Error {
    let reason = res
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    Rejected(res.status().as_u16(), reason).into()
}

fn bearer() -> String {
    "Bearer".into()
}

impl Session {
    pub fn is_dpop(&self) -> bool {
        self.token_type == "DPoP"
    }

    /// Renew within a minute of expiry (web: `DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS`).
    pub fn needs_renewal(&self, now_ms: i64) -> bool {
        self.is_dpop() && self.expires_at.is_some_and(|at| at - now_ms < 60_000)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
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

const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";

/// `/oauth/token`: a pairing credential (`bootstrap`) or, for DPoP sessions, the current access
/// token renewing itself.
async fn exchange(
    base_url: &str,
    subject: &str,
    subject_type: &str,
    dpop: bool,
) -> Result<Session> {
    #[derive(Deserialize)]
    struct TokenResult {
        access_token: String,
        token_type: String,
        expires_in: f64,
    }
    let url = format!("{base_url}/oauth/token");
    let mut request = reqwest::Client::new().post(&url).form(&[
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange",
        ),
        ("subject_token", subject),
        ("subject_token_type", subject_type),
        ("requested_token_type", ACCESS_TOKEN_TYPE),
        ("client_label", "T3 Code (Rust)"),
        ("client_device_type", "desktop"),
    ]);
    if dpop {
        request = request.header("dpop", crate::dpop::proof("POST", &url, None));
    }
    let res = request.send().await?;
    if matches!(res.status().as_u16(), 401 | 403) && subject_type == ACCESS_TOKEN_TYPE {
        return Err(rejected(res));
    }
    if !res.status().is_success() {
        bail!(
            "token exchange failed ({}): {}",
            res.status(),
            res.text().await.unwrap_or_default()
        );
    }
    let token: TokenResult = res.json().await?;
    Ok(Session {
        base_url: base_url.to_owned(),
        access_token: token.access_token,
        token_type: token.token_type,
        expires_at: Some(now_ms() + (token.expires_in * 1000.0) as i64),
    })
}

/// The environment descriptor's `capabilities`, or null when it cannot be read.
pub async fn capabilities(base_url: &str) -> serde_json::Value {
    let read = async {
        reqwest::Client::new()
            .get(format!("{base_url}/.well-known/t3/environment"))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await
    };
    read.await
        .map(|d| d["capabilities"].clone())
        .unwrap_or_default()
}

/// Pairs with a DPoP key when the server can renew such sessions (`accessTokenRenewal`),
/// else with a bearer token, as the capability's contract asks.
/// Pair against the server's own port: the Vite dev proxy rewrites Host for `/api`
/// (`packages/shared/src/devProxy.ts`), so DPoP proofs for tickets fail through it.
pub async fn pair(pairing_url: &str) -> Result<Session> {
    let (base_url, credential) = parse_pairing_url(pairing_url)?;
    let descriptor: serde_json::Value = reqwest::Client::new()
        .get(format!("{base_url}/.well-known/t3/environment"))
        .send()
        .await?
        .json()
        .await
        .unwrap_or_default();
    let dpop = descriptor["capabilities"]["accessTokenRenewal"] == true;
    let session = exchange(
        &base_url,
        &credential,
        "urn:t3:params:oauth:token-type:environment-bootstrap",
        dpop,
    )
    .await?;
    save(&session);
    Ok(session)
}

/// Renews a DPoP session in place and saves it.
pub async fn renew(session: &Session) -> Result<Session> {
    let renewed = exchange(
        &session.base_url,
        &session.access_token,
        ACCESS_TOKEN_TYPE,
        true,
    )
    .await?;
    save(&renewed);
    Ok(renewed)
}

/// The server refused the saved session; retrying cannot help, the user must pair again.
#[derive(Debug)]
pub struct Rejected(pub u16, pub String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The server rejected this session ({}{}). Pair again.",
            self.0,
            if self.1.is_empty() {
                String::new()
            } else {
                format!(": {}", self.1)
            }
        )
    }
}

impl std::error::Error for Rejected {}

/// A socket URL with a fresh ticket. DPoP sessions prove the key on the ticket request and on
/// the upgrade (in the `dpop` query parameter, since a WebSocket cannot set headers).
pub async fn socket_url(session: &Session) -> Result<String> {
    #[derive(Deserialize)]
    struct Ticket {
        ticket: String,
    }
    let ticket_url = format!("{}/api/auth/websocket-ticket/rpc", session.base_url);
    let mut request = reqwest::Client::new().post(&ticket_url);
    request = if session.is_dpop() {
        request
            .header("authorization", format!("DPoP {}", session.access_token))
            .header(
                "dpop",
                crate::dpop::proof("POST", &ticket_url, Some(&session.access_token)),
            )
    } else {
        request.bearer_auth(&session.access_token)
    };
    let res = request.send().await?;
    if matches!(res.status().as_u16(), 401 | 403) {
        return Err(rejected(res));
    }
    if !res.status().is_success() {
        bail!("server returned {}", res.status());
    }
    let ticket: Ticket = res.json().await?;
    let mut url = Url::parse(&session.base_url)?;
    let secure = url.scheme() == "https";
    url.set_scheme(if secure { "wss" } else { "ws" }).ok();
    url.set_path("/ws");
    url.query_pairs_mut()
        .append_pair("wsTicket", &ticket.ticket)
        .append_pair("orchestrationProtocol", "2")
        .append_pair("frameParts", "1");
    if session.is_dpop() {
        let upgrade = format!(
            "{}://{}/ws",
            if secure { "https" } else { "http" },
            url.authority()
        );
        url.query_pairs_mut().append_pair(
            "dpop",
            &crate::dpop::proof("GET", &upgrade, Some(&ticket.ticket)),
        );
    }
    Ok(url.into())
}

/// An authenticated GET to the environment's HTTP API (Bearer, or DPoP with a proof).
pub async fn get_json(session: &Session, path_and_query: &str) -> Result<serde_json::Value> {
    let url = format!("{}{path_and_query}", session.base_url);
    let mut request = reqwest::Client::new()
        .get(&url)
        .header("x-t3-orchestration-protocol", "2");
    request = if session.is_dpop() {
        request
            .header("authorization", format!("DPoP {}", session.access_token))
            .header(
                "dpop",
                crate::dpop::proof("GET", &url, Some(&session.access_token)),
            )
    } else {
        request.bearer_auth(&session.access_token)
    };
    let res = request.send().await?;
    if matches!(res.status().as_u16(), 401 | 403) {
        return Err(rejected(res));
    }
    if !res.status().is_success() {
        bail!(
            "{} returned {}",
            path_and_query.split('?').next().unwrap_or(""),
            res.status()
        );
    }
    Ok(res.json().await?)
}

/// An authenticated JSON POST to the environment's HTTP API (Bearer, or DPoP with a proof).
pub async fn post_json(
    session: &Session,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let url = format!("{}{path}", session.base_url);
    let mut request = reqwest::Client::new().post(&url).json(body);
    request = if session.is_dpop() {
        request
            .header("authorization", format!("DPoP {}", session.access_token))
            .header(
                "dpop",
                crate::dpop::proof("POST", &url, Some(&session.access_token)),
            )
    } else {
        request.bearer_auth(&session.access_token)
    };
    let res = request.send().await?;
    if matches!(res.status().as_u16(), 401 | 403) {
        return Err(rejected(res));
    }
    if !res.status().is_success() {
        bail!(
            "{path} returned {}: {}",
            res.status(),
            res.text().await.unwrap_or_default()
        );
    }
    Ok(res.json().await.unwrap_or(serde_json::Value::Null))
}

fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/t3-gpui"))
}

/// One session per environment, keyed by server URL.
fn sessions_path() -> Option<PathBuf> {
    Some(config_dir()?.join("sessions.json"))
}

/// The single-session file this client wrote before it supported several environments.
fn legacy_session_path() -> Option<PathBuf> {
    Some(config_dir()?.join("session.json"))
}

/// Saved sessions; `T3_SERVER` + `T3_TOKEN` replace them for development.
pub fn load_all() -> Vec<Session> {
    if let (Ok(base_url), Ok(access_token)) =
        (std::env::var("T3_SERVER"), std::env::var("T3_TOKEN"))
    {
        let token_type = std::env::var("T3_TOKEN_TYPE").unwrap_or_else(|_| bearer());
        return vec![Session {
            base_url,
            access_token,
            token_type,
            expires_at: None,
        }];
    }
    let read = |p: Option<PathBuf>| std::fs::read(p?).ok();
    if let Some(bytes) = read(sessions_path()) {
        return serde_json::from_slice(&bytes).unwrap_or_default();
    }
    read(legacy_session_path())
        .and_then(|b| serde_json::from_slice::<Session>(&b).ok())
        .into_iter()
        .collect()
}

fn write_all(sessions: &[Session]) {
    let Some(path) = sessions_path() else { return };
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(&path, serde_json::to_vec(sessions).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    if let Some(legacy) = legacy_session_path() {
        let _ = std::fs::remove_file(legacy);
    }
}

pub fn upsert(sessions: &mut Vec<Session>, session: Session) {
    sessions.retain(|s| s.base_url != session.base_url);
    sessions.push(session);
}

pub fn save(session: &Session) {
    let mut all = load_all();
    upsert(&mut all, session.clone());
    write_all(&all);
}

pub fn forget(base_url: &str) {
    let mut all = load_all();
    all.retain(|s| s.base_url != base_url);
    write_all(&all);
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_pairing_url() {
        let (base, token) =
            super::parse_pairing_url("http://192.168.1.4:3773/pair#token=abc%2B1").unwrap();
        assert_eq!(base, "http://192.168.1.4:3773");
        assert_eq!(token, "abc+1");
        assert!(super::parse_pairing_url("http://x/pair").is_err());
    }

    #[test]
    fn upsert_replaces_by_server() {
        let s = |url: &str, token: &str| super::Session {
            base_url: url.into(),
            access_token: token.into(),
            token_type: "Bearer".into(),
            expires_at: None,
        };
        let mut all = vec![s("http://a", "1"), s("http://b", "2")];
        super::upsert(&mut all, s("http://a", "3"));
        assert_eq!(
            all.iter()
                .map(|s| (s.base_url.as_str(), s.access_token.as_str()))
                .collect::<Vec<_>>(),
            [("http://b", "2"), ("http://a", "3")]
        );
    }

    #[test]
    fn dpop_sessions_renew_near_expiry_and_old_files_load_as_bearer() {
        let old: super::Session =
            serde_json::from_str(r#"{"base_url":"http://a","access_token":"t"}"#).unwrap();
        assert!(!old.is_dpop() && !old.needs_renewal(0));
        let dpop = super::Session {
            base_url: "http://a".into(),
            access_token: "t".into(),
            token_type: "DPoP".into(),
            expires_at: Some(100_000),
        };
        assert!(!dpop.needs_renewal(0));
        assert!(dpop.needs_renewal(50_000));
    }
}
