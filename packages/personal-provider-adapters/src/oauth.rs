//! Device-local OAuth. Desktop loopback PKCE; Android must use Google's supported
//! Android authorization SDK, not this desktop redirect/client type.
use crate::{ensure, google::bounded_json, Result, SCOPES_READ};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN: &str = "https://oauth2.googleapis.com/token";
pub const REVOKE: &str = "https://oauth2.googleapis.com/revoke";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthConfig {
    pub client_id: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}
impl OAuthConfig {
    pub fn validate_desktop(&self) -> Result<Url> {
        ensure(
            self.client_id.ends_with(".apps.googleusercontent.com")
                && self.client_id.len() <= 256
                && self
                    .client_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c)),
            "UNCONFIGURED: supply your Google Desktop OAuth client ID",
        )?;
        ensure(
            self.client_secret
                .as_ref()
                .is_none_or(|s| s.len() <= 1024 && !s.chars().any(|c| c.is_control())),
            "Invalid optional client credential",
        )?;
        let uri = Url::parse(&self.redirect_uri).map_err(|_| "Invalid redirect")?;
        ensure(uri.scheme()=="http" && uri.host_str()==Some("127.0.0.1") && uri.port().is_some_and(|p| p >= 1024) && uri.path()=="/oauth/callback" && uri.query().is_none() && uri.fragment().is_none() && uri.username().is_empty() && uri.password().is_none(), "Desktop redirect must be http://127.0.0.1:<port>/oauth/callback, registered for your client")?;
        Ok(uri)
    }
}
#[derive(Serialize, Deserialize)]
pub struct Tokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_ms: u64,
    pub(crate) scopes: Vec<String>,
    pub(crate) account: String,
}
impl Drop for Tokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
impl Tokens {
    pub fn account(&self) -> &str {
        &self.account
    }
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
    pub(crate) fn require(&self, scope: &str) -> Result<()> {
        ensure(
            self.scopes.iter().any(|s| s == scope),
            "Provider permission missing; authorize this specific operation first",
        )
    }
}
pub struct PendingOAuth {
    verifier: String,
    state: String,
    pub authorization_url: String,
    config: OAuthConfig,
    created_ms: u64,
    requested: Vec<String>,
}
impl Drop for PendingOAuth {
    fn drop(&mut self) {
        self.verifier.zeroize();
        self.state.zeroize();
    }
}
fn random() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
impl PendingOAuth {
    pub fn begin(config: OAuthConfig, write_scopes: bool, now: u64) -> Result<Self> {
        config.validate_desktop()?;
        let verifier = random();
        let state = random();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut requested = SCOPES_READ
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if write_scopes {
            requested.extend(
                ["gmail.compose", "gmail.modify", "calendar.events"]
                    .iter()
                    .map(|s| format!("https://www.googleapis.com/auth/{s}")),
            );
        }
        let mut url = Url::parse(AUTH).map_err(|_| "Authorization URL unavailable")?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", &requested.join(" ")),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
        ]);
        Ok(Self {
            verifier,
            state,
            authorization_url: url.to_string(),
            config,
            created_ms: now,
            requested,
        })
    }
    /// Callback is consumed once by the bound loopback listener; never passed through
    /// a webview, logs, model context, clipboard or peer sync.
    pub async fn finish(self, callback: &str, now: u64) -> Result<Tokens> {
        ensure(
            now >= self.created_ms && now - self.created_ms < 180_000,
            "OAuth session expired",
        )?;
        let url = Url::parse(callback).map_err(|_| "Invalid OAuth callback")?;
        let expected = self.config.validate_desktop()?;
        ensure(
            url.origin() == expected.origin()
                && url.path() == expected.path()
                && url.fragment().is_none(),
            "Callback redirect mismatch",
        )?;
        let pairs = url.query_pairs().collect::<Vec<_>>();
        let only = |key: &str| -> Result<String> {
            let values = pairs
                .iter()
                .filter(|(k, _)| k == key)
                .map(|(_, v)| v.to_string())
                .collect::<Vec<_>>();
            ensure(
                values.len() == 1,
                "Missing/duplicate OAuth response parameter",
            )?;
            Ok(values[0].clone())
        };
        ensure(only("state")? == self.state, "OAuth state mismatch")?;
        ensure(
            !pairs.iter().any(|(k, _)| k == "error"),
            "Google authorization declined",
        )?;
        let code = zeroize::Zeroizing::new(only("code")?);
        let mut form = vec![
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", self.verifier.as_str()),
        ];
        if let Some(secret) = self.config.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }
        let response = crate::google::client()?
            .post(TOKEN)
            .form(&form)
            .send()
            .await
            .map_err(|_| "OAuth exchange failed; reconnect, never log token response")?;
        let value = bounded_json(response, 64 * 1024).await?;
        let mut tokens = parse_tokens(value, now, None)?;
        ensure(
            self.requested
                .iter()
                .all(|scope| tokens.scopes.contains(scope)),
            "Not all requested permissions were granted; no account connected",
        )?;
        let profile = crate::google::profile(&tokens).await?;
        crate::types::address(&profile)?;
        tokens.account = profile;
        Ok(tokens)
    }
}
fn parse_tokens(
    mut value: serde_json::Value,
    now: u64,
    previous: Option<&Tokens>,
) -> Result<Tokens> {
    let access = value["access_token"]
        .as_str()
        .ok_or("OAuth token response missing access token")?
        .to_string();
    ensure(
        access.len() <= 8192
            && !access.is_empty()
            && value["token_type"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("Bearer")),
        "Unsupported OAuth token",
    )?;
    let refresh = value["refresh_token"]
        .as_str()
        .map(str::to_string)
        .or_else(|| previous.map(|p| p.refresh_token.clone()))
        .unwrap_or_default();
    let scopes = value["scope"]
        .as_str()
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .or_else(|| previous.map(|p| p.scopes.clone()))
        .ok_or("OAuth scope response missing")?;
    let seconds = value["expires_in"].as_u64().ok_or("OAuth expiry missing")?;
    ensure(
        seconds > 0 && seconds <= 86400 && refresh.len() <= 8192,
        "Invalid token expiry/size",
    )?;
    for key in ["access_token", "refresh_token", "id_token"] {
        if let Some(serde_json::Value::String(s)) = value.get_mut(key) {
            s.zeroize();
        }
    } // Never format/log provider credential values.
    value.take();
    Ok(Tokens {
        access_token: access,
        refresh_token: refresh,
        expires_ms: now + seconds * 1000,
        scopes,
        account: previous.map(|p| p.account.clone()).unwrap_or_default(),
    })
}
pub async fn refresh(config: &OAuthConfig, tokens: &mut Tokens, now: u64) -> Result<()> {
    if now + 60_000 < tokens.expires_ms {
        return Ok(());
    }
    config.validate_desktop()?;
    ensure(
        !tokens.refresh_token.is_empty(),
        "Authorization expired; reconnect account",
    )?;
    let mut form = vec![
        ("client_id", config.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", tokens.refresh_token.as_str()),
    ];
    if let Some(secret) = config.client_secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let response = crate::google::client()?
        .post(TOKEN)
        .form(&form)
        .send()
        .await
        .map_err(|_| "Refresh unavailable; no provider operation dispatched")?;
    let next = parse_tokens(bounded_json(response, 64 * 1024).await?, now, Some(tokens))?;
    // Rebind refreshed token by reading provider profile, not trusting a caller email.
    ensure(
        crate::google::profile(&next).await? == tokens.account,
        "Account binding mismatch; disconnect",
    )?;
    *tokens = next;
    Ok(())
}
pub async fn revoke(tokens: &Tokens) -> Result<()> {
    let token = if tokens.refresh_token.is_empty() {
        &tokens.access_token
    } else {
        &tokens.refresh_token
    };
    let response = crate::google::client()?
        .post(REVOKE)
        .form(&[("token", token.as_str())])
        .send()
        .await
        .map_err(|_| "Remote revocation not confirmed; remove access in Google Account settings")?;
    ensure(
        response.status().is_success(),
        "Remote revocation not confirmed; remove access in Google Account settings",
    )
}
