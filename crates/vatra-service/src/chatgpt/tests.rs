use super::*;
use serde_json::json;
use std::sync::Arc;
use vatra_core::MemorySecretStore;

fn signed(claims: Value) -> String {
    let key = signature::RsaKeyPair::from_pkcs8(include_bytes!("fixtures/test-key.der")).unwrap();
    let head = B64.encode(br#"{"alg":"RS256","kid":"synthetic"}"#);
    let body = B64.encode(serde_json::to_vec(&claims).unwrap());
    let input = format!("{head}.{body}");
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &signature::RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{input}.{}", B64.encode(signature))
}
fn keys() -> Value {
    serde_json::from_str(include_str!("fixtures/test-jwks.json")).unwrap()
}

#[test]
fn signed_identity_validates_signature_issuer_audience_time_and_nonce() {
    let claims = json!({"iss":ISSUER,"aud":["oaiapp_test"],"sub":"synthetic-user","nonce":"expected","iat":now(),"exp":now()+3600});
    let token = signed(claims.clone());
    assert!(verify_id(&token, &keys(), ISSUER, "oaiapp_test", Some("expected")).is_ok());
    assert!(verify_id(&token, &keys(), ISSUER, "wrong", Some("expected")).is_err());
    assert!(verify_id(&token, &keys(), ISSUER, "oaiapp_test", Some("wrong")).is_err());
    for (key, value) in [
        ("iss", json!("https://evil.test")),
        ("exp", json!(now() - 10)),
        ("iat", json!(now() + 3600)),
        ("sub", json!("")),
    ] {
        let mut altered = claims.clone();
        altered[key] = value;
        assert!(
            verify_id(
                &signed(altered),
                &keys(),
                ISSUER,
                "oaiapp_test",
                Some("expected")
            )
            .is_err()
        );
    }
    let mut parts: Vec<_> = token.split('.').map(str::to_owned).collect();
    parts[1] = B64.encode(br#"{"sub":"forged"}"#);
    assert!(verify_id(&parts.join("."), &keys(), ISSUER, "oaiapp_test", None).is_err());
}

#[test]
fn callback_rejects_wrong_state_duplicate_params_and_changed_client() {
    assert!(
        callback(
            "GET /auth/callback?state=bad&code=c&client_id=oaiapp_test HTTP/1.1",
            "good",
            None
        )
        .is_err()
    );
    assert!(
        callback(
            "GET /auth/callback?state=good&state=good&code=c&client_id=oaiapp_test HTTP/1.1",
            "good",
            None
        )
        .is_err()
    );
    assert!(
        callback(
            "GET /auth/callback?state=good&code=c&client_id=oaiapp_other HTTP/1.1",
            "good",
            Some("oaiapp_test")
        )
        .is_err()
    );
    assert!(matches!(
        callback(
            "GET /auth/callback?state=good&error=access_denied HTTP/1.1",
            "good",
            None
        ),
        Err(AuthError::Cancelled)
    ));
    assert_eq!(
        callback(
            "GET /auth/callback?state=good&code=c HTTP/1.1",
            "good",
            Some("oaiapp_test")
        )
        .unwrap()["client_id"],
        "oaiapp_test"
    );
}

#[tokio::test]
async fn status_answers_while_a_sign_in_waits_for_the_browser() {
    let dir = tempfile::tempdir().unwrap();
    let auth = ChatGptAuth::new(dir.path(), Arc::new(MemorySecretStore::default())).unwrap();
    let held = auth.gate.lock().await;
    let marked = Connecting::mark(&auth.connecting);
    let status = tokio::time::timeout(Duration::from_secs(1), auth.status())
        .await
        .expect("status must not wait for the sign-in")
        .unwrap();
    assert!(status.connecting && !status.connected);
    drop(marked);
    drop(held);
    assert!(!auth.status().await.unwrap().connecting);
}

#[tokio::test]
async fn identity_only_and_uncertain_refresh_never_authorize_inference() {
    let dir = tempfile::tempdir().unwrap();
    let auth = ChatGptAuth::new(dir.path(), Arc::new(MemorySecretStore::default())).unwrap();
    let mut record = Record {
        client_id: "oaiapp_test".into(),
        subject: Some("subject".into()),
        ..Default::default()
    };
    update(
        &mut record,
        Tokens {
            access_token: String::new(),
            refresh_token: None,
            id_token: Some("synthetic-validated-id".into()),
            token_type: String::new(),
            expires_in: 0,
            scope: Some("openid email".into()),
        },
        false,
    )
    .unwrap();
    auth.save(&record).unwrap();
    assert!(auth.status().await.unwrap().connected);
    assert!(!auth.status().await.unwrap().sharing);
    assert!(matches!(
        auth.access_token_for_child().await,
        Err(AuthError::Permission)
    ));
    record.refresh_pending = true;
    auth.save(&record).unwrap();
    assert!(matches!(
        auth.access_token_for_child().await,
        Err(AuthError::SignInRequired)
    ));
}

#[tokio::test]
async fn browser_code_exchange_refresh_and_logout_use_only_vatra_storage() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::default());
    let mut auth = ChatGptAuth::new(dir.path(), secrets.clone()).unwrap();
    auth.issuer = server.uri();
    let auth = Arc::new(auth);
    Mock::given(method("GET")).and(path("/.well-known/openid-configuration")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"issuer":server.uri(),"authorization_endpoint":format!("{}/authorize",server.uri()),"token_endpoint":format!("{}/token",server.uri()),"jwks_uri":format!("{}/keys",server.uri()),"revocation_endpoint":format!("{}/revoke",server.uri())}))).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(keys()))
        .mount(&server)
        .await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task_auth = auth.clone();
    let task = tokio::spawn(async move {
        task_auth
            .connect(
                &|url| {
                    tx.send(url).unwrap();
                },
                Cancellation::new(),
            )
            .await
    });
    let address = match rx.recv().await { Some(address)=>address, None=>panic!("sign-in ended before browser callback: {:?}",task.await.unwrap()) };
    let url = Url::parse(&address).unwrap();
    let params: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["client_id"], "dynamic_agent_client");
    assert_eq!(params["agent_name_hint"], "Vatra");
    assert_eq!(params["code_challenge_method"], "S256");
    let id = signed(
        json!({"iss":server.uri(),"aud":"oaiapp_test","sub":"user","email":"test@example.invalid","nonce":params["nonce"],"iat":now(),"exp":now()+3600}),
    );
    Mock::given(method("POST")).and(path("/token")).and(body_string_contains("grant_type=authorization_code")).and(body_string_contains("client_id=oaiapp_test")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"synthetic-access","refresh_token":"synthetic-refresh","id_token":id,"token_type":"Bearer","expires_in":1,"scope":SCOPE}))).mount(&server).await;
    let mut callback_url = Url::parse(&params["redirect_uri"]).unwrap();
    callback_url.query_pairs_mut().extend_pairs([
        ("code", "synthetic-code"),
        ("state", &params["state"]),
        ("client_id", "oaiapp_test"),
    ]);
    reqwest::get(callback_url).await.unwrap();
    task.await.unwrap().unwrap();
    assert!(auth.status().await.unwrap().sharing);
    Mock::given(method("POST")).and(path("/token")).and(body_string_contains("grant_type=refresh_token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"rotated-access","refresh_token":"rotated-refresh","token_type":"Bearer","expires_in":3600,"scope":SCOPE}))).mount(&server).await;
    let token = auth.access_token_for_child().await.unwrap();
    let mut child = tokio::process::Command::new("synthetic");
    token.apply_to(&mut child);
    assert_eq!(
        child
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "ACCESS_TOKEN")
            .unwrap()
            .1
            .unwrap(),
        "rotated-access"
    );
    assert!(child.as_std().get_args().next().is_none());
    let record = auth.load().unwrap();
    assert_eq!(record.refresh_token.as_deref(), Some("rotated-refresh"));
    for file in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(file.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("access"));
    }
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert!(auth.disconnect().await.unwrap());
    assert!(!auth.status().await.unwrap().connected);
}

#[test]
fn provider_is_selected_before_startup_and_never_has_token_in_argv() {
    let command = app_server_command(
        Path::new("codex.exe"),
        &["--listen".into(), "stdio://".into()],
    );
    let args: Vec<_> = command
        .as_std()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "app-server");
    assert!(args.contains(&"model_provider=\"openai_chatgpt_plan\"".into()));
    assert!(
        args.iter()
            .any(|s| s.ends_with("requires_openai_auth=false"))
    );
    assert!(!args.iter().any(|s| s.contains("synthetic-access")));
    assert!(
        !command
            .as_std()
            .get_envs()
            .any(|(key, _)| key == "CODEX_HOME")
    );
}

#[test]
#[ignore = "writes a disposable synthetic entry to the actual OS keychain"]
fn actual_os_keychain_round_trip() {
    use vatra_core::{KeyringBackend, KeyringStore};
    let store = KeyringStore::new(KeyringBackend);
    let key = format!("siwc-test:{}", uuid::Uuid::new_v4());
    store.set(&key, "synthetic-test-value").unwrap();
    let value = store.get(&key);
    store.delete(&key).unwrap();
    assert_eq!(value.unwrap().as_deref(), Some("synthetic-test-value"));
    assert!(store.get(&key).unwrap().is_none());
}

#[test]
fn host_is_persistent_and_only_metadata_is_written() {
    let directory = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::default());
    let first = ChatGptAuth::new(directory.path(), secrets.clone()).unwrap();
    let id = first.host.host_id.clone();
    assert!(id.starts_with("urn:uuid:"));
    let second = ChatGptAuth::new(directory.path(), secrets.clone()).unwrap();
    assert_eq!(second.host.host_id, id);
    assert!(secrets.keys().is_empty());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[tokio::test]
async fn absent_credentials_are_disconnected_and_cancellation_is_immediate() {
    let directory = tempfile::tempdir().unwrap();
    let auth = ChatGptAuth::new(directory.path(), Arc::new(MemorySecretStore::default())).unwrap();
    assert!(!auth.status().await.unwrap().connected);
    assert!(matches!(
        auth.access_token_for_child().await,
        Err(AuthError::SignInRequired)
    ));
    let cancelled = Cancellation::new();
    cancelled.cancel();
    assert!(matches!(
        auth.connect(&|_| panic!("must not open browser"), cancelled)
            .await,
        Err(AuthError::Cancelled)
    ));
}
#[tokio::test]
async fn accounts_list_and_select_one_active() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = std::sync::Arc::new(vatra_core::MemorySecretStore::default());
    let auth = super::ChatGptAuth::new(dir.path(), secrets).unwrap();
    auth.save(&super::Record { subject:Some("a".into()),email:Some("a@example.test".into()),id_token:Some("synthetic-a".into()), ..Default::default() }).unwrap();
    auth.save(&super::Record { subject:Some("b".into()),email:Some("b@example.test".into()),id_token:Some("synthetic-b".into()), ..Default::default() }).unwrap();
    let accounts = auth.accounts().await.unwrap();
    assert_eq!(accounts.len(),2);
    assert_eq!(accounts.iter().filter(|a|a.active).count(),1);
    auth.select("a").await.unwrap();
    assert_eq!(auth.status().await.unwrap().account.as_deref(),Some("a@example.test"));
    assert!(auth.select("unknown").await.is_err());
}
