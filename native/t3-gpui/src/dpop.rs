//! DPoP proofs (RFC 9449) bound to this device's ES256 key, like the web/mobile clients
//! (`packages/shared/src/dpop.ts`, `client-runtime/src/authorization/remote.ts`).

use std::path::PathBuf;
use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use serde_json::json;
use sha2::{Digest, Sha256};

fn key_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".config/t3-gpui/dpop-key.pem"))
}

/// The device key: loaded once, created and saved (0600) on first use.
pub fn device_key() -> &'static SigningKey {
    static KEY: OnceLock<SigningKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let path = key_path();
        if let Some(key) = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|pem| SigningKey::from_pkcs8_pem(&pem).ok())
        {
            return key;
        }
        let key = SigningKey::random(&mut rand_core::OsRng);
        if let (Some(path), Ok(pem)) = (path, key.to_pkcs8_pem(LineEnding::LF)) {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(&path, pem.as_bytes());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
        key
    })
}

/// `ath`: base64url SHA-256 of the access token (or WebSocket ticket) the proof covers.
pub fn token_hash(token: &str) -> String {
    B64.encode(Sha256::digest(token.as_bytes()))
}

/// `normalizeDpopHtu`: the URL without query or fragment.
pub fn htu(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_owned()
}

/// A signed `dpop+jwt` for one request.
pub fn proof_with(
    key: &SigningKey,
    method: &str,
    url: &str,
    access_token: Option<&str>,
    iat: i64,
    jti: &str,
) -> String {
    let point = key.verifying_key().to_encoded_point(false);
    let jwk = json!({
        "kty": "EC",
        "crv": "P-256",
        "x": B64.encode(point.x().expect("uncompressed point")),
        "y": B64.encode(point.y().expect("uncompressed point")),
    });
    let header = json!({ "typ": "dpop+jwt", "alg": "ES256", "jwk": jwk });
    let mut payload = json!({ "htm": method, "htu": htu(url), "jti": jti, "iat": iat });
    if let Some(token) = access_token {
        payload["ath"] = json!(token_hash(token));
    }
    let signing_input = format!(
        "{}.{}",
        B64.encode(header.to_string()),
        B64.encode(payload.to_string())
    );
    let signature: Signature = key.sign(signing_input.as_bytes());
    format!("{signing_input}.{}", B64.encode(signature.to_bytes()))
}

pub fn proof(method: &str, url: &str, access_token: Option<&str>) -> String {
    let iat = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    proof_with(
        device_key(),
        method,
        url,
        access_token,
        iat,
        &uuid::Uuid::new_v4().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};

    #[test]
    fn proofs_verify_and_carry_the_claims() {
        let key = SigningKey::random(&mut rand_core::OsRng);
        let jwt = proof_with(
            &key,
            "POST",
            "https://h:1/api/auth/websocket-ticket/rpc?x=1#y",
            Some("tok"),
            1_700_000_000,
            "id-1",
        );
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[0]).unwrap()).unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(
            (
                header["typ"].as_str(),
                header["alg"].as_str(),
                header["jwk"]["crv"].as_str()
            ),
            (Some("dpop+jwt"), Some("ES256"), Some("P-256"))
        );
        assert!(header["jwk"].get("d").is_none(), "never the private key");
        assert_eq!(payload["htu"], "https://h:1/api/auth/websocket-ticket/rpc");
        assert_eq!(
            (
                payload["htm"].as_str(),
                payload["iat"].as_i64(),
                payload["jti"].as_str()
            ),
            (Some("POST"), Some(1_700_000_000), Some("id-1"))
        );
        assert_eq!(payload["ath"], token_hash("tok"));
        let signature = Signature::from_slice(&B64.decode(parts[2]).unwrap()).unwrap();
        let verifying = VerifyingKey::from(&key);
        assert!(
            verifying
                .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
                .is_ok()
        );
    }
}
