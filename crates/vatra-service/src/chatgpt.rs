//! Vatra-owned public-client OAuth. No native harness credentials or third-party SDK code.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, watch},
};
use url::Url;
use vatra_core::SecretStore;

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Connect ChatGPT in Settings before starting this agent.")]
    SignInRequired,
    #[error("ChatGPT sign-in was cancelled.")]
    Cancelled,
    #[error("Another ChatGPT account operation is already running.")]
    Busy,
    #[error("ChatGPT sign-in could not be verified. Try connecting again.")]
    Invalid,
    #[error("ChatGPT could not be reached. Try again.")]
    Network,
    #[error("Vatra could not use protected account storage.")]
    Storage,
    #[error(
        "ChatGPT plan usage is not enabled for this connection. Connect again and allow plan usage."
    )]
    Permission,
}
type Result<T> = std::result::Result<T, AuthError>;

#[derive(Clone)]
pub struct Cancellation(watch::Sender<bool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}
impl Cancellation {
    pub fn new() -> Self {
        Self(watch::channel(false).0)
    }
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        if *rx.borrow() {
            return;
        }
        let _ = rx.changed().await;
    }
    fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
}

#[derive(Clone)]
pub struct ChildAccessToken(String);
impl ChildAccessToken {
    pub fn apply_to(&self, command: &mut tokio::process::Command) {
        command.env("ACCESS_TOKEN", &self.0);
    }
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Status {
    pub connected: bool,
    pub sharing: bool,
    pub account: Option<String>,
    /// A sign-in is waiting for the browser. The rest of the status isn't read until it ends.
    pub connecting: bool,
}
#[derive(Serialize, Deserialize)]
struct Host {
    host_id: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Record {
    client_id: String,
    subject: Option<String>,
    email: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: String,
    expires_at: u64,
    #[serde(default)]
    refresh_pending: bool,
}
#[derive(Serialize)]
pub struct AccountView { pub id: String, pub account: Option<String>, pub active: bool }
#[derive(Deserialize)]
struct Tokens {
    #[serde(default)]
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: u64,
    scope: Option<String>,
}
#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}

pub struct ChatGptAuth {
    host: Host,
    key: String,
    lock_path: PathBuf,
    secrets: Arc<dyn SecretStore>,
    gate: Mutex<()>,
    connecting: AtomicBool,
    http: reqwest::Client,
    issuer: String,
}
/// Marks a sign-in as running until it ends, also when its future is dropped.
struct Connecting<'a>(&'a AtomicBool);
impl<'a> Connecting<'a> {
    fn mark(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Relaxed);
        Self(flag)
    }
}
impl Drop for Connecting<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
impl ChatGptAuth {
    pub fn new(directory: &Path, secrets: Arc<dyn SecretStore>) -> Result<Self> {
        fs::create_dir_all(directory).map_err(|_| AuthError::Storage)?;
        let lock_path = directory.join("chatgpt-operation.lock");
        let _lock = process_lock(&lock_path)?;
        let host_file = directory.join("chatgpt-host.json");
        let host = match fs::read(&host_file) {
            Ok(bytes) => serde_json::from_slice::<Host>(&bytes).map_err(|_| AuthError::Storage)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let host = Host {
                    host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
                };
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&host_file)
                    .map_err(|_| AuthError::Storage)?;
                std::io::Write::write_all(
                    &mut file,
                    &serde_json::to_vec(&host).map_err(|_| AuthError::Storage)?,
                )
                .map_err(|_| AuthError::Storage)?;
                file.sync_all().map_err(|_| AuthError::Storage)?;
                host
            }
            Err(_) => return Err(AuthError::Storage),
        };
        if !host.host_id.starts_with("urn:uuid:") {
            return Err(AuthError::Storage);
        }
        let key = format!("chatgpt:{}", host.host_id);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| AuthError::Network)?;
        Ok(Self {
            host,
            key,
            lock_path,
            secrets,
            gate: Mutex::new(()),
            connecting: AtomicBool::new(false),
            http,
            issuer: ISSUER.into(),
        })
    }
    fn load(&self) -> Result<Record> {
        self.secrets
            .get(&self.key)
            .map_err(|_| AuthError::Storage)?
            .map(|text| serde_json::from_str(&text).map_err(|_| AuthError::Storage))
            .unwrap_or_else(|| Ok(Record::default()))
    }
    fn save(&self, record: &Record) -> Result<()> {
        let mut accounts = self.saved_accounts()?;
        if let Some(id) = &record.subject {
            if record.id_token.is_some() { accounts.insert(id.clone(),record.clone()); }
            else { accounts.remove(id); }
        }
        self.secrets.set(&format!("{}:accounts",self.key), &serde_json::to_string(&accounts).map_err(|_|AuthError::Storage)?)
            .map_err(|_|AuthError::Storage)?;
        self.secrets
            .set(
                &self.key,
                &serde_json::to_string(record).map_err(|_| AuthError::Storage)?,
            )
            .map_err(|_| AuthError::Storage)
    }
    fn saved_accounts(&self) -> Result<BTreeMap<String, Record>> {
        self.secrets.get(&format!("{}:accounts",self.key)).map_err(|_|AuthError::Storage)?
            .map(|text|serde_json::from_str(&text).map_err(|_|AuthError::Storage)).unwrap_or_else(||Ok(BTreeMap::new()))
    }
    pub async fn accounts(&self) -> Result<Vec<AccountView>> {
        let _guard = self.gate.try_lock().map_err(|_|AuthError::Busy)?;
        let current = self.load()?;
        let mut accounts = self.saved_accounts()?;
        if let Some(id) = &current.subject && current.id_token.is_some() { accounts.insert(id.clone(),current.clone()); }
        Ok(accounts.into_iter().map(|(id,r)|AccountView { active:current.subject.as_ref()==Some(&id),id,account:r.email }).collect())
    }
    pub async fn select(&self, id: &str) -> Result<()> {
        let _guard = self.gate.try_lock().map_err(|_|AuthError::Busy)?;
        let _file = process_lock(&self.lock_path)?;
        let mut accounts = self.saved_accounts()?;
        let current = self.load()?;
        if let Some(id) = &current.subject && current.id_token.is_some() { accounts.insert(id.clone(),current); }
        let selected = accounts.get(id).ok_or(AuthError::SignInRequired)?;
        // Both current and historical records remain in protected storage only.
        self.secrets.set(&format!("{}:accounts",self.key),&serde_json::to_string(&accounts).map_err(|_|AuthError::Storage)?).map_err(|_|AuthError::Storage)?;
        self.save(selected)
    }
    pub async fn status(&self) -> Result<Status> {
        // A sign-in holds the gate while it waits for the browser, for minutes.
        if self.connecting.load(Ordering::Relaxed) {
            return Ok(Status {
                connecting: true,
                ..Status::default()
            });
        }
        let _guard = self.gate.lock().await;
        let r = self.load()?;
        Ok(Status {
            connecting: false,
            connected: r.id_token.is_some() && r.subject.is_some() && !r.refresh_pending,
            sharing: r.access_token.is_some()
                && r.scope.split_whitespace().any(|s| s == PLAN_SCOPE)
                && !r.refresh_pending,
            account: r.email,
        })
    }
    async fn discovery(&self) -> Result<Discovery> {
        let d: Discovery = self
            .get_json(&format!("{}/.well-known/openid-configuration", self.issuer))
            .await?;
        if d.issuer != self.issuer {
            return Err(AuthError::Invalid);
        }
        for endpoint in [&d.authorization_endpoint, &d.token_endpoint, &d.jwks_uri]
            .into_iter()
            .chain(d.revocation_endpoint.iter())
        {
            let url = Url::parse(endpoint).map_err(|_| AuthError::Invalid)?;
            let issuer = Url::parse(&self.issuer).map_err(|_| AuthError::Invalid)?;
            if url.origin() != issuer.origin() || url.username() != "" || url.password().is_some() {
                return Err(AuthError::Invalid);
            }
        }
        Ok(d)
    }
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.http
            .get(url)
            .send()
            .await
            .map_err(|_| AuthError::Network)?
            .error_for_status()
            .map_err(|_| AuthError::Network)?
            .json()
            .await
            .map_err(|_| AuthError::Invalid)
    }
    pub async fn connect(
        &self,
        open_url: &(dyn Fn(String) + Send + Sync),
        cancel: Cancellation,
    ) -> Result<()> {
        let _guard = self.gate.try_lock().map_err(|_| AuthError::Busy)?;
        let _connecting = Connecting::mark(&self.connecting);
        let _file = process_lock(&self.lock_path)?;
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        tokio::select! {biased; _=cancel.cancelled()=>Err(AuthError::Cancelled), result=tokio::time::timeout(Duration::from_secs(300),self.connect_inner(open_url))=>result.map_err(|_|AuthError::Cancelled)?}
    }
    async fn connect_inner(&self, open_url: &(dyn Fn(String) + Send + Sync)) -> Result<()> {
        let discovery = self.discovery().await?;
        let old = self.load()?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| AuthError::Network)?;
        let redirect = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener
                .local_addr()
                .map_err(|_| AuthError::Network)?
                .port()
        );
        let state = random()?;
        let nonce = random()?;
        let verifier = random()?;
        let dynamic = old.client_id.is_empty();
        let client = if dynamic {
            "dynamic_agent_client"
        } else {
            &old.client_id
        };
        let mut url =
            Url::parse(&discovery.authorization_endpoint).map_err(|_| AuthError::Invalid)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.extend_pairs([
                ("client_id", client),
                ("ext_agent_host_id", self.host.host_id.as_str()),
                ("response_type", "code"),
                ("redirect_uri", redirect.as_str()),
                ("scope", SCOPE),
                ("resource", RESOURCE),
                ("state", state.as_str()),
                ("nonce", nonce.as_str()),
                ("code_challenge_method", "S256"),
                (
                    "code_challenge",
                    B64.encode(Sha256::digest(verifier.as_bytes())).as_str(),
                ),
            ]);
            if dynamic {
                pairs.append_pair("agent_name_hint", "Vatra");
            } else if let Some(id) = &old.id_token {
                pairs.append_pair("id_token_hint", id);
            }
        }
        open_url(url.into());
        let callback = loop {
            let (mut stream, _) = listener.accept().await.map_err(|_| AuthError::Network)?;
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                let mut buffer = Vec::new();
                let mut part = [0; 1024];
                while buffer.len() < 8192 && !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream
                        .read(&mut part)
                        .await
                        .map_err(|_| AuthError::Invalid)?;
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&part[..n]);
                }
                let line = std::str::from_utf8(&buffer)
                    .map_err(|_| AuthError::Invalid)?
                    .lines()
                    .next()
                    .ok_or(AuthError::Invalid)?;
                callback(
                    line,
                    &state,
                    if dynamic { None } else { Some(&old.client_id) },
                )
            })
            .await;
            let valid = matches!(&result, Ok(Ok(_)));
            let reply = if valid {
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nReturn to Vatra to finish connecting."
            } else {
                "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nInvalid callback."
            };
            let _ =
                tokio::time::timeout(Duration::from_secs(2), stream.write_all(reply.as_bytes()))
                    .await;
            if matches!(&result, Ok(Err(AuthError::Cancelled))) {
                return Err(AuthError::Cancelled);
            }
            if let Ok(Ok(params)) = result {
                break params;
            }
        };
        let mut pending = old;
        pending.client_id = callback["client_id"].clone();
        // Retain an issued registration even if a one-time authorization code expires.
        self.save(&pending)?;
        let tokens = self
            .exchange(
                &discovery.token_endpoint,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &pending.client_id),
                    ("code", &callback["code"]),
                    ("code_verifier", &verifier),
                    ("redirect_uri", &redirect),
                    ("resource", RESOURCE),
                ],
            )
            .await?;
        let id = tokens.id_token.as_ref().ok_or(AuthError::Invalid)?;
        let keys: Value = self.get_json(&discovery.jwks_uri).await?;
        let identity = verify_id(id, &keys, &self.issuer, &pending.client_id, Some(&nonce))?;
        // A new interactive connection may select another account. Refresh still validates the subject.
        if pending.subject.as_deref() != identity["sub"].as_str() {
            // Never carry a previous account's refresh token into a newly selected identity.
            pending = Record { client_id:pending.client_id.clone(), ..Default::default() };
        }
        pending.subject = identity["sub"].as_str().map(str::to_owned);
        pending.email = identity["email"].as_str().map(str::to_owned);
        update(&mut pending, tokens, false)?;
        self.save(&pending)
    }
    async fn exchange(&self, endpoint: &str, form: &[(&str, &str)]) -> Result<Tokens> {
        let response = self
            .http
            .post(endpoint)
            .form(form)
            .send()
            .await
            .map_err(|_| AuthError::Network)?;
        if !response.status().is_success() {
            return Err(AuthError::SignInRequired);
        }
        response.json().await.map_err(|_| AuthError::Invalid)
    }
    pub async fn access_token_for_child(&self) -> Result<ChildAccessToken> {
        let _guard = self.gate.try_lock().map_err(|_| AuthError::Busy)?;
        let _file = process_lock(&self.lock_path)?;
        let mut r = self.load()?;
        if r.refresh_pending || r.id_token.is_none() {
            return Err(AuthError::SignInRequired);
        }
        if !r.scope.split_whitespace().any(|s| s == PLAN_SCOPE) {
            return Err(AuthError::Permission);
        }
        if r.expires_at <= now() + 120 {
            let refresh = r.refresh_token.clone().ok_or(AuthError::SignInRequired)?;
            let d = self.discovery().await?;
            // A transport failure may have consumed a rotating token. Never blindly replay it.
            r.refresh_pending = true;
            self.save(&r)?;
            let tokens = self
                .exchange(
                    &d.token_endpoint,
                    &[
                        ("grant_type", "refresh_token"),
                        ("client_id", &r.client_id),
                        ("refresh_token", &refresh),
                        ("resource", RESOURCE),
                    ],
                )
                .await?;
            if let Some(id) = &tokens.id_token {
                let keys: Value = self.get_json(&d.jwks_uri).await?;
                let identity = verify_id(id, &keys, &self.issuer, &r.client_id, None)?;
                if identity["sub"].as_str() != r.subject.as_deref() {
                    return Err(AuthError::Invalid);
                }
            }
            update(&mut r, tokens, true)?;
            self.save(&r)?;
        }
        if !r.scope.split_whitespace().any(|s| s == PLAN_SCOPE) {
            return Err(AuthError::Permission);
        }
        Ok(ChildAccessToken(
            r.access_token.ok_or(AuthError::SignInRequired)?,
        ))
    }
    /// Returns whether remote revocation was confirmed; local credentials are always cleared.
    pub async fn disconnect(&self) -> Result<bool> {
        let _guard = self.gate.try_lock().map_err(|_| AuthError::Busy)?;
        let _file = process_lock(&self.lock_path)?;
        let mut r = self.load()?;
        let mut confirmed = r.refresh_token.is_none();
        if let Some(refresh) = &r.refresh_token
            && let Ok(d) = self.discovery().await
            && let Some(endpoint) = d.revocation_endpoint
        {
            confirmed = self
                .http
                .post(endpoint)
                .form(&[
                    ("token", refresh.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", r.client_id.as_str()),
                ])
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
        }
        r.access_token = None;
        r.refresh_token = None;
        r.id_token = None;
        r.scope.clear();
        r.expires_at = 0;
        r.refresh_pending = false;
        self.save(&r)?;
        Ok(confirmed)
    }
}
fn process_lock(path: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| AuthError::Storage)?;
    file.try_lock().map_err(|_| AuthError::Busy)?;
    Ok(file)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random() -> Result<String> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| AuthError::Invalid)?;
    Ok(B64.encode(bytes))
}
fn update(r: &mut Record, t: Tokens, refresh: bool) -> Result<()> {
    let sharing = t
        .scope
        .as_deref()
        .unwrap_or(&r.scope)
        .split_whitespace()
        .any(|s| s == PLAN_SCOPE);
    if (!t.access_token.is_empty()
        && (!t.token_type.eq_ignore_ascii_case("bearer") || t.expires_in == 0))
        || (sharing && t.access_token.is_empty())
        || (refresh && t.refresh_token.as_ref().is_none_or(|s| s.is_empty()))
    {
        return Err(AuthError::Invalid);
    }
    r.access_token = (!t.access_token.is_empty()).then_some(t.access_token);
    r.refresh_token = t.refresh_token.or(r.refresh_token.take());
    r.id_token = t.id_token.or(r.id_token.take());
    if let Some(scope) = t.scope {
        r.scope = scope;
    } else if !refresh {
        r.scope.clear();
    }
    r.expires_at = now().saturating_add(t.expires_in);
    r.refresh_pending = false;
    Ok(())
}
fn callback(line: &str, state: &str, client: Option<&str>) -> Result<BTreeMap<String, String>> {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("GET") {
        return Err(AuthError::Invalid);
    }
    let target = parts.next().ok_or(AuthError::Invalid)?;
    if !target.starts_with("/auth/callback?") {
        return Err(AuthError::Invalid);
    }
    let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| AuthError::Invalid)?;
    let mut pairs = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        if pairs.insert(k.into_owned(), v.into_owned()).is_some() {
            return Err(AuthError::Invalid);
        }
    }
    if pairs.get("state").map(String::as_str) != Some(state) {
        return Err(AuthError::Invalid);
    }
    if pairs.get("error").is_some_and(|s| s == "access_denied") {
        return Err(AuthError::Cancelled);
    }
    if pairs.contains_key("error") || pairs.get("code").is_none_or(|s| s.is_empty()) {
        return Err(AuthError::Invalid);
    }
    match (client, pairs.get("client_id")) {
        (Some(expected), Some(actual)) if expected != actual => return Err(AuthError::Invalid),
        (Some(expected), None) => {
            pairs.insert("client_id".into(), expected.into());
        }
        (None, Some(issued)) if issued.starts_with("oaiapp_") => {}
        (None, _) => return Err(AuthError::Invalid),
        _ => {}
    }
    Ok(pairs)
}
fn verify_id(
    token: &str,
    keys: &Value,
    issuer: &str,
    client: &str,
    nonce: Option<&str>,
) -> Result<Value> {
    if token.len() > 64 * 1024 {
        return Err(AuthError::Invalid);
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthError::Invalid);
    }
    let decode = |p: &str| -> Result<Vec<u8>> { B64.decode(p).map_err(|_| AuthError::Invalid) };
    let header: Value =
        serde_json::from_slice(&decode(parts[0])?).map_err(|_| AuthError::Invalid)?;
    if header["alg"] != "RS256" || header.get("crit").is_some() {
        return Err(AuthError::Invalid);
    }
    let kid = header["kid"].as_str().ok_or(AuthError::Invalid)?;
    let key = keys["keys"]
        .as_array()
        .ok_or(AuthError::Invalid)?
        .iter()
        .find(|k| {
            k["kid"] == kid
                && k["kty"] == "RSA"
                && k.get("use").is_none_or(|v| v == "sig")
                && k.get("alg").is_none_or(|v| v == "RS256")
        })
        .ok_or(AuthError::Invalid)?;
    let n = decode(key["n"].as_str().ok_or(AuthError::Invalid)?)?;
    let e = decode(key["e"].as_str().ok_or(AuthError::Invalid)?)?;
    signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &decode(parts[2])?,
        )
        .map_err(|_| AuthError::Invalid)?;
    let c: Value = serde_json::from_slice(&decode(parts[1])?).map_err(|_| AuthError::Invalid)?;
    let audience = c["aud"].as_str() == Some(client)
        || c["aud"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(client)));
    let party_ok = c.get("azp").is_none_or(|v| v.as_str() == Some(client))
        && (c["aud"].as_array().is_none_or(|a| a.len() <= 1) || c["azp"].as_str() == Some(client));
    if c["iss"].as_str() != Some(issuer)
        || !audience
        || !party_ok
        || c["sub"].as_str().is_none_or(|s| s.is_empty())
        || c["exp"].as_u64().is_none_or(|t| t <= now())
        || c["iat"].as_u64().is_none_or(|t| t > now() + 5)
        || c.get("nbf")
            .is_some_and(|v| v.as_u64().is_none_or(|t| t > now() + 5))
        || nonce.is_some_and(|n| c["nonce"].as_str() != Some(n))
    {
        return Err(AuthError::Invalid);
    }
    Ok(c)
}

/// Provider selection must happen at process start, before codex-acp performs account/read.
fn app_server_command(binary: &Path, args: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command.arg("app-server").args(args);
    for setting in [
        "model_provider=\"openai_chatgpt_plan\"",
        "model_providers.openai_chatgpt_plan.name=\"ChatGPT plan\"",
        "model_providers.openai_chatgpt_plan.base_url=\"https://api.openai.com/v1\"",
        "model_providers.openai_chatgpt_plan.env_key=\"ACCESS_TOKEN\"",
        "model_providers.openai_chatgpt_plan.wire_api=\"responses\"",
        "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
        "model_providers.openai_chatgpt_plan.supports_websockets=false",
    ] {
        command.arg("-c").arg(setting);
    }
    command
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}
pub async fn run_app_server(args: &[String]) -> std::result::Result<(), String> {
    if std::env::var_os("ACCESS_TOKEN").is_none_or(|s| s.is_empty()) {
        return Err("The Vatra ChatGPT connection is missing. Reconnect in Settings.".into());
    }
    let binary = std::env::var_os("VATRA_CODEX_BINARY")
        .map(PathBuf::from)
        .ok_or_else(|| "The official Codex executable is not configured.".to_string())?;
    if fs::canonicalize(&binary)
        .ok()
        .zip(
            std::env::current_exe()
                .ok()
                .and_then(|p| fs::canonicalize(p).ok()),
        )
        .is_some_and(|(a, b)| a == b)
    {
        return Err("The Codex executable cannot be Vatra itself.".into());
    }
    let status = app_server_command(&binary, args)
        .status()
        .await
        .map_err(|_| "The official Codex executable could not be started.".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Codex app-server stopped unsuccessfully.".into())
    }
}
#[cfg(test)]
mod tests;
