use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use futures::TryStreamExt;
use reqwest::{Client, Proxy};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::Arc};
use tokio::sync::RwLock;

const HOP_BY_HOP_HEADERS: [&str; 6] = [
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "transfer-encoding",
];

const PROFILE_AUTH_HEADERS: [&str; 4] = [
    "authorization",
    "x-api-key",
    "chatgpt-account-id",
    "openai-organization",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub listen: String,
    #[serde(default)]
    pub outbound_proxy: Option<String>,
    pub default_profile: String,
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub bindings: BTreeMap<String, Binding>,
    #[serde(default)]
    pub session_tags: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Profile {
    pub base_url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Binding {
    pub mode: BindingMode,
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BindingMode {
    Global,
    Fixed,
}

#[derive(Debug, Clone)]
pub struct ProxyState {
    config: Arc<RwLock<Config>>,
    client: Arc<RwLock<Client>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionIdSource {
    Header,
    TurnMetadataThreadId,
    None,
}

impl Config {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read config {}: {error}", path.display()))?;
        let config = serde_json::from_str::<Self>(&text)
            .map_err(|error| format!("parse config {}: {error}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.listen.trim().is_empty() {
            return Err("listen must not be empty".to_string());
        }
        if let Some(proxy_url) = outbound_proxy_url(self) {
            Proxy::all(proxy_url).map_err(|error| format!("outbound_proxy is invalid: {error}"))?;
        }
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(format!(
                "default profile {:?} is not configured",
                self.default_profile
            ));
        }

        for (profile_id, profile) in &self.profiles {
            let url = profile.base_url.trim_end_matches('/');
            let parsed = reqwest::Url::parse(url)
                .map_err(|error| format!("profile {profile_id:?} has invalid base_url: {error}"))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(format!(
                    "profile {profile_id:?} base_url must use http or https"
                ));
            }
            for header_name in profile.headers.keys() {
                HeaderName::from_bytes(header_name.as_bytes()).map_err(|error| {
                    format!("profile {profile_id:?} has invalid header {header_name:?}: {error}")
                })?;
            }
            for (header_name, header_value) in &profile.headers {
                HeaderValue::from_str(header_value).map_err(|error| {
                    format!(
                        "profile {profile_id:?} has invalid value for header {header_name:?}: {error}"
                    )
                })?;
            }
        }

        for (session_id, binding) in &self.bindings {
            if binding.mode == BindingMode::Fixed {
                let Some(profile_id) = binding.profile.as_deref() else {
                    return Err(format!(
                        "fixed binding for session {session_id:?} requires a profile"
                    ));
                };
                if !self.profiles.contains_key(profile_id) {
                    return Err(format!(
                        "binding for session {session_id:?} references unknown profile {profile_id:?}"
                    ));
                }
            }
        }

        self.listen
            .parse::<SocketAddr>()
            .map_err(|error| format!("listen must be a socket address: {error}"))?;
        Ok(())
    }
}

impl ProxyState {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        let client = build_http_client(&config)?;
        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            client: Arc::new(RwLock::new(client)),
        })
    }

    pub async fn replace_config(&self, config: Config) -> Result<(), String> {
        config.validate()?;
        let client = build_http_client(&config)?;
        *self.config.write().await = config;
        *self.client.write().await = client;
        Ok(())
    }

    async fn route_for(&self, headers: &HeaderMap) -> Result<(String, Profile), String> {
        let config = self.config.read().await;
        let session_id = extract_session_id(headers).0;
        let profile_id = match session_id.as_deref() {
            Some(session_id) => match config.bindings.get(session_id) {
                Some(binding) if binding.mode == BindingMode::Fixed => binding
                    .profile
                    .clone()
                    .ok_or_else(|| format!("fixed binding for {session_id:?} has no profile"))?,
                _ => config.default_profile.clone(),
            },
            None => config.default_profile.clone(),
        };
        let profile = config
            .profiles
            .get(&profile_id)
            .ok_or_else(|| format!("profile {profile_id:?} is not configured"))?;

        Ok((
            profile.base_url.trim_end_matches('/').to_string(),
            profile.clone(),
        ))
    }
}

fn outbound_proxy_url(config: &Config) -> Option<&str> {
    config
        .outbound_proxy
        .as_deref()
        .map(str::trim)
        .filter(|proxy_url| !proxy_url.is_empty())
}

fn build_http_client(config: &Config) -> Result<Client, String> {
    let mut builder = Client::builder();
    if let Some(proxy_url) = outbound_proxy_url(config) {
        builder = builder.proxy(
            Proxy::all(proxy_url).map_err(|error| format!("outbound_proxy is invalid: {error}"))?,
        );
    }
    builder
        .build()
        .map_err(|error| format!("build HTTP client: {error}"))
}

fn profile_uses_client_auth(profile: &Profile) -> bool {
    // ponytail: ChatGPT OAuth tokens are short-lived; Codex owns refresh, so use its request headers.
    profile.base_url.contains("chatgpt.com/backend-api/codex")
        || profile
            .headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("chatgpt-account-id"))
}

pub fn app(state: ProxyState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .fallback(proxy)
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn proxy(State(state): State<ProxyState>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    let method = parts.method;
    let uri = parts.uri;
    let headers = parts.headers;
    let (upstream, profile) = match state.route_for(&headers).await {
        Ok(route) => route,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let upstream_url = format!("{upstream}{path_and_query}");
    let request_method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("unsupported HTTP method: {error}"),
            )
        }
    };

    let mut upstream_headers = headers;
    for header_name in HOP_BY_HOP_HEADERS {
        upstream_headers.remove(header_name);
    }
    let uses_client_auth = profile_uses_client_auth(&profile);
    if !uses_client_auth {
        for header_name in PROFILE_AUTH_HEADERS {
            upstream_headers.remove(header_name);
        }
    }
    for (header_name, header_value) in &profile.headers {
        let name = match HeaderName::from_bytes(header_name.as_bytes()) {
            Ok(name) => name,
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("invalid configured header {header_name:?}: {error}"),
                )
            }
        };
        if uses_client_auth
            && PROFILE_AUTH_HEADERS
                .iter()
                .any(|auth_header| name.as_str().eq_ignore_ascii_case(auth_header))
        {
            continue;
        }
        let value = match HeaderValue::from_str(header_value) {
            Ok(value) => value,
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("invalid configured header value {header_name:?}: {error}"),
                )
            }
        };
        upstream_headers.insert(name, value);
    }

    let client = state.client.read().await.clone();
    let upstream_response = client
        .request(request_method, &upstream_url)
        .headers(upstream_headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream().map_err(
            |error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()),
        )))
        .send()
        .await;

    let upstream_response = match upstream_response {
        Ok(response) => response,
        Err(error) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                format!("upstream request failed: {error}"),
            )
        }
    };

    let status = upstream_response.status();
    let response_status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::builder().status(response_status);
    let response_headers = response.headers_mut().expect("response builder is valid");
    for (header_name, header_value) in upstream_response.headers() {
        if HOP_BY_HOP_HEADERS
            .iter()
            .any(|hop_header| header_name.as_str().eq_ignore_ascii_case(hop_header))
        {
            continue;
        }
        response_headers.append(header_name.clone(), header_value.clone());
    }
    let response = response
        .body(Body::from_stream(upstream_response.bytes_stream()))
        .expect("response body should be constructible");
    response
}

fn error_response(status: StatusCode, message: String) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "error": message
        })),
    )
        .into_response()
}

pub fn extract_session_id(headers: &HeaderMap) -> (Option<String>, SessionIdSource) {
    // ponytail: route from headers only; parsing the body would buffer streaming Codex requests.
    for header_name in ["session_id", "x-session-id"] {
        if let Some(value) = headers
            .get(header_name)
            .and_then(|value| value.to_str().ok())
        {
            let value = value.trim();
            if !value.is_empty() {
                return (Some(value.to_string()), SessionIdSource::Header);
            }
        }
    }

    if let Some(thread_id) = headers
        .get("x-codex-turn-metadata")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| {
            let thread_id = value
                .get("threadId")
                .or_else(|| value.get("thread_id"))
                .and_then(Value::as_str)?
                .trim();
            (!thread_id.is_empty()).then(|| thread_id.to_string())
        })
    {
        return (Some(thread_id), SessionIdSource::TurnMetadataThreadId);
    }

    (None, SessionIdSource::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        response::IntoResponse,
        routing::any,
    };
    use futures::StreamExt;
    use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
    use tokio::{
        sync::{oneshot, Notify},
        task::JoinHandle,
        time::timeout,
    };

    struct TestServer {
        addr: SocketAddr,
        task: JoinHandle<()>,
    }

    impl TestServer {
        async fn start(router: Router) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind test server");
            let addr = listener.local_addr().expect("test server address");
            let task = tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("test server should run");
            });
            Self { addr, task }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn upstream_handler(
        State(profile): State<String>,
        request: Request<Body>,
    ) -> impl IntoResponse {
        let session_id = request
            .headers()
            .get("session_id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("none");
        (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "profile": profile,
                "session_id": session_id,
            })),
        )
    }

    async fn auth_echo_handler(request: Request<Body>) -> impl IntoResponse {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("missing");
        let account_id = request
            .headers()
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("missing");
        (
            StatusCode::OK,
            axum::Json(serde_json::json!({
                "authorization": authorization,
                "account_id": account_id,
            })),
        )
    }

    async fn start_upstream(profile: &str) -> TestServer {
        TestServer::start(
            Router::new()
                .fallback(any(upstream_handler))
                .with_state(profile.to_string()),
        )
        .await
    }

    fn test_config(upstreams: &HashMap<&str, TestServer>) -> Config {
        let profiles = upstreams
            .iter()
            .map(|(profile, server)| {
                (
                    (*profile).to_string(),
                    Profile {
                        base_url: format!("http://{}", server.addr),
                        headers: BTreeMap::from([(
                            "x-poc-upstream".to_string(),
                            (*profile).to_string(),
                        )]),
                    },
                )
            })
            .collect();
        Config {
            listen: "127.0.0.1:0".to_string(),
            outbound_proxy: None,
            default_profile: "sakura".to_string(),
            profiles,
            bindings: BTreeMap::from([
                (
                    "session-a".to_string(),
                    Binding {
                        mode: BindingMode::Fixed,
                        profile: Some("aihezu".to_string()),
                    },
                ),
                (
                    "session-b".to_string(),
                    Binding {
                        mode: BindingMode::Fixed,
                        profile: Some("her".to_string()),
                    },
                ),
                (
                    "session-c".to_string(),
                    Binding {
                        mode: BindingMode::Global,
                        profile: None,
                    },
                ),
            ]),
            session_tags: BTreeMap::new(),
        }
    }

    async fn collect<T>(futures: Vec<T>) -> Vec<T::Output>
    where
        T: Future,
    {
        futures::future::join_all(futures).await
    }

    #[test]
    fn session_id_extraction_uses_declared_priority() {
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("from-session"));
        headers.insert("x-session-id", HeaderValue::from_static("from-header"));
        assert_eq!(
            extract_session_id(&headers),
            (Some("from-session".to_string()), SessionIdSource::Header)
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"threadId":"from-metadata"}"#),
        );
        assert_eq!(
            extract_session_id(&headers),
            (
                Some("from-metadata".to_string()),
                SessionIdSource::TurnMetadataThreadId
            )
        );
    }

    #[test]
    fn session_id_extraction_reads_codex_turn_metadata() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"threadId":"from-codex","turnId":"turn-1"}"#),
        );
        assert_eq!(
            extract_session_id(&headers),
            (
                Some("from-codex".to_string()),
                SessionIdSource::TurnMetadataThreadId
            )
        );
    }

    #[test]
    fn config_without_session_tags_defaults_to_empty() {
        let config: Config = serde_json::from_str(
            r#"{
                "listen": "127.0.0.1:8787",
                "default_profile": "sakura",
                "profiles": {
                    "sakura": {
                        "base_url": "https://api.example.com/v1"
                    }
                }
            }"#,
        )
        .expect("legacy config should deserialize");
        assert!(config.session_tags.is_empty());
        assert_eq!(config.outbound_proxy, None);
    }

    #[tokio::test]
    async fn replace_config_rebuilds_the_outbound_proxy_client() {
        let direct_upstream = TestServer::start(Router::new().fallback(any(|| async {
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({"route": "direct"})),
            )
        })))
        .await;
        let outbound_proxy = TestServer::start(Router::new().fallback(any(
            |request: Request<Body>| async move {
                (
                    StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "route": "outbound_proxy",
                        "uri": request.uri().to_string(),
                    })),
                )
            },
        )))
        .await;

        let mut config = Config {
            listen: "127.0.0.1:0".to_string(),
            outbound_proxy: None,
            default_profile: "personal".to_string(),
            profiles: BTreeMap::from([(
                "personal".to_string(),
                Profile {
                    base_url: format!("http://{}", direct_upstream.addr),
                    headers: BTreeMap::new(),
                },
            )]),
            bindings: BTreeMap::new(),
            session_tags: BTreeMap::new(),
        };
        let state = ProxyState::new(config.clone()).expect("valid test config");
        let proxy = TestServer::start(app(state.clone())).await;
        let client = reqwest::Client::new();

        let response = client
            .get(format!("http://{}/models", proxy.addr))
            .send()
            .await
            .expect("direct proxy request")
            .json::<Value>()
            .await
            .expect("direct response");
        assert_eq!(response["route"], "direct");

        config.outbound_proxy = Some(format!("http://{}", outbound_proxy.addr));
        state
            .replace_config(config)
            .await
            .expect("replace config with outbound proxy");

        let response = client
            .get(format!("http://{}/models", proxy.addr))
            .send()
            .await
            .expect("outbound proxy request")
            .json::<Value>()
            .await
            .expect("outbound proxy response");
        assert_eq!(response["route"], "outbound_proxy");
        assert!(response["uri"].as_str().unwrap().ends_with("/models"));
    }

    #[tokio::test]
    async fn proxy_routes_concurrent_sessions_to_their_profiles() {
        let mut upstreams = HashMap::new();
        upstreams.insert("aihezu", start_upstream("aihezu").await);
        upstreams.insert("her", start_upstream("her").await);
        upstreams.insert("sakura", start_upstream("sakura").await);

        let state = ProxyState::new(test_config(&upstreams)).expect("valid test config");
        let proxy = TestServer::start(app(state)).await;
        let client = reqwest::Client::new();
        let proxy_url = format!("http://{}", proxy.addr);

        let mut requests = Vec::new();
        for _ in 0..10 {
            for session_id in ["session-a", "session-b", "session-c"] {
                let client = client.clone();
                let url = format!("{proxy_url}/responses");
                requests.push(async move {
                    let response = client
                        .post(url)
                        .header("session_id", session_id)
                        .json(&serde_json::json!({"input": "poc"}))
                        .send()
                        .await
                        .expect("proxy request");
                    assert_eq!(response.status(), StatusCode::OK);
                    response
                        .json::<Value>()
                        .await
                        .expect("upstream JSON response")
                });
            }
        }

        let responses = collect(requests).await;
        let mut counts = BTreeMap::<String, usize>::new();
        for response in responses {
            *counts
                .entry(response["profile"].as_str().unwrap().to_string())
                .or_default() += 1;
        }
        assert_eq!(
            counts,
            BTreeMap::from([
                ("aihezu".to_string(), 10),
                ("her".to_string(), 10),
                ("sakura".to_string(), 10),
            ])
        );

        let response = client
            .post(format!("{proxy_url}/v1/responses"))
            .header(
                "x-codex-turn-metadata",
                r#"{"threadId":"session-b","turnId":"turn-1"}"#,
            )
            .json(&serde_json::json!({
                "input": "poc"
            }))
            .send()
            .await
            .expect("metadata session request");
        assert_eq!(response.json::<Value>().await.unwrap()["profile"], "her");

        let response = client
            .post(format!("{proxy_url}/responses"))
            .header(
                "x-codex-turn-metadata",
                r#"{"threadId":"session-b","turnId":"turn-1"}"#,
            )
            .json(&serde_json::json!({"input": "poc"}))
            .send()
            .await
            .expect("Codex turn metadata request");
        assert_eq!(response.json::<Value>().await.unwrap()["profile"], "her");

        let response = client
            .get(format!("{proxy_url}/models"))
            .send()
            .await
            .expect("default profile request");
        assert_eq!(response.json::<Value>().await.unwrap()["profile"], "sakura");
    }

    #[tokio::test]
    async fn proxy_uses_client_auth_for_chatgpt_profiles() {
        let upstream = TestServer::start(Router::new().fallback(any(auth_echo_handler))).await;
        let config = Config {
            listen: "127.0.0.1:0".to_string(),
            outbound_proxy: None,
            default_profile: "personal".to_string(),
            profiles: BTreeMap::from([(
                "personal".to_string(),
                Profile {
                    base_url: format!("http://{}", upstream.addr),
                    headers: BTreeMap::from([
                        (
                            "authorization".to_string(),
                            "Bearer expired-profile-token".to_string(),
                        ),
                        (
                            "chatgpt-account-id".to_string(),
                            "expired-profile-account".to_string(),
                        ),
                    ]),
                },
            )]),
            bindings: BTreeMap::new(),
            session_tags: BTreeMap::new(),
        };
        let state = ProxyState::new(config).expect("valid test config");
        let proxy = TestServer::start(app(state)).await;
        let response = reqwest::Client::new()
            .post(format!("http://{}/responses", proxy.addr))
            .header("authorization", "Bearer current-client-token")
            .header("chatgpt-account-id", "current-client-account")
            .send()
            .await
            .expect("proxy request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<Value>().await.expect("auth echo response");
        assert_eq!(body["authorization"], "Bearer current-client-token");
        assert_eq!(body["account_id"], "current-client-account");
    }

    #[tokio::test]
    async fn proxy_uses_profile_auth_for_api_key_profiles() {
        let upstream = TestServer::start(Router::new().fallback(any(auth_echo_handler))).await;
        let config = Config {
            listen: "127.0.0.1:0".to_string(),
            outbound_proxy: None,
            default_profile: "sakura".to_string(),
            profiles: BTreeMap::from([(
                "sakura".to_string(),
                Profile {
                    base_url: format!("http://{}", upstream.addr),
                    headers: BTreeMap::from([(
                        "authorization".to_string(),
                        "Bearer profile-api-key".to_string(),
                    )]),
                },
            )]),
            bindings: BTreeMap::new(),
            session_tags: BTreeMap::new(),
        };
        let state = ProxyState::new(config).expect("valid test config");
        let proxy = TestServer::start(app(state)).await;
        let response = reqwest::Client::new()
            .get(format!("http://{}/models", proxy.addr))
            .header("authorization", "Bearer current-client-token")
            .header("chatgpt-account-id", "current-client-account")
            .send()
            .await
            .expect("proxy request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<Value>().await.expect("auth echo response");
        assert_eq!(body["authorization"], "Bearer profile-api-key");
        assert_eq!(body["account_id"], "missing");
    }

    #[tokio::test]
    async fn proxy_forwards_request_body_before_the_body_ends() {
        let first_chunk_received = Arc::new(Notify::new());
        let upstream_signal = first_chunk_received.clone();
        let streaming_upstream =
            TestServer::start(Router::new().fallback(any(move |body: Body| {
                let upstream_signal = upstream_signal.clone();
                async move {
                    if body
                        .into_data_stream()
                        .next()
                        .await
                        .is_some_and(|chunk| chunk.is_ok())
                    {
                        upstream_signal.notify_one();
                    }
                    StatusCode::OK
                }
            })))
            .await;

        let mut upstreams = HashMap::new();
        upstreams.insert("aihezu", streaming_upstream);
        upstreams.insert("her", start_upstream("her").await);
        upstreams.insert("sakura", start_upstream("sakura").await);
        let state = ProxyState::new(test_config(&upstreams)).expect("valid test config");
        let proxy = TestServer::start(app(state)).await;
        let client = reqwest::Client::new();
        let proxy_url = format!("http://{}", proxy.addr);

        let (release_tx, release_rx) = oneshot::channel();
        let mut release_rx = Some(release_rx);
        let body = futures::stream::unfold(0, move |step| {
            let gate = if step == 1 { release_rx.take() } else { None };
            async move {
                match step {
                    0 => Some((Ok::<_, std::io::Error>(b"first".as_slice()), 1)),
                    1 => {
                        gate.expect("release gate").await.ok();
                        Some((Ok(b"second".as_slice()), 2))
                    }
                    _ => None,
                }
            }
        });

        let request = tokio::spawn(
            client
                .post(format!("{proxy_url}/responses"))
                .header("session_id", "session-a")
                .body(reqwest::Body::wrap_stream(body))
                .send(),
        );

        timeout(Duration::from_millis(250), first_chunk_received.notified())
            .await
            .expect("proxy buffered the request body before forwarding it");
        release_tx.send(()).expect("release request body");

        let response = request
            .await
            .expect("proxy request task")
            .expect("proxy request");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
