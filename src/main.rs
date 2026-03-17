use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::Response,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{env, fs, sync::Arc};
use tokio::net::TcpListener;

#[derive(Debug, serde::Deserialize)]
pub struct FileConfig {
    pub upstream_url: String,
    pub bind_addr: Option<String>,
    pub injections: Vec<InjectionRule>,
}

#[derive(Debug, serde::Deserialize)]
pub struct InjectionRule {
    pub header: String,
    pub contains: Vec<String>,
    pub links_source: String,
}

pub struct AppState {
    pub upstream: String,
    pub bind_addr: String,
    pub injections: Vec<InjectionRule>,
    pub client: reqwest::Client,
}

pub fn load_config(path: &str) -> FileConfig {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Cannot read config file {path}: {e}"));
    toml::from_str(&content)
        .unwrap_or_else(|e| panic!("Invalid config file {path}: {e}"))
}

pub fn find_matching_rule<'a>(headers: &HeaderMap, rules: &'a [InjectionRule]) -> Option<&'a InjectionRule> {
    for rule in rules {
        let header_name = rule.header.to_lowercase();
        let header_val = headers
            .get(header_name.as_str())
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        if rule.contains.iter().any(|pat| header_val.contains(pat.as_str())) {
            return Some(rule);
        }
    }
    None
}

pub async fn load_extra_links_from_source(source: &str, client: &reqwest::Client) -> String {
    if source.starts_with("http://") || source.starts_with("https://") {
        match client.get(source).send().await {
            Ok(resp) => {
                if resp.content_length().unwrap_or(0) > MAX_BODY_SIZE {
                    eprintln!("[sub-injector] links source response too large: {source}");
                    return String::new();
                }
                let text = resp.text().await.unwrap_or_default();
                if text.len() as u64 > MAX_BODY_SIZE {
                    eprintln!("[sub-injector] links source response too large: {source}");
                    return String::new();
                }
                text.trim().to_string()
            }
            Err(e) => {
                eprintln!("[sub-injector] failed to fetch links from {source}: {e}");
                String::new()
            }
        }
    } else {
        fs::read_to_string(source)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

pub fn inject_links(body: &[u8], extra: &str) -> Vec<u8> {
    // Strip whitespace (newlines) before decoding — some upstreams wrap base64 at 76 chars
    let stripped: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    let decoded = match STANDARD.decode(&stripped) {
        Ok(d) => d,
        Err(_) => return body.to_vec(),
    };
    let text = match String::from_utf8(decoded) {
        Ok(t) => t,
        Err(_) => return body.to_vec(),
    };
    let combined = format!("{}\n{}", text.trim(), extra);
    STANDARD.encode(combined.as_bytes()).into_bytes()
}

const MAX_BODY_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

pub async fn proxy(
    State(cfg): State<Arc<AppState>>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response<axum::body::Body>, StatusCode> {
    let upstream_url = format!(
        "{}{}",
        cfg.upstream,
        uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
    );

    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/").to_string();
    let ua_preview = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").chars().take(40).collect::<String>();
    // Log only the first two path segments to avoid leaking subscription tokens
    let path_preview = uri.path().splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
    eprintln!("[sub-injector] >> GET {path_preview}/... ua={ua_preview:?}");

    let mut req = cfg.client.get(&upstream_url);

    for (name, value) in &headers {
        if name.as_str().to_lowercase() == "connection" {
            continue;
        }
        req = req.header(name.as_str(), value.to_str().unwrap_or(""));
    }

    let resp = req.send().await.map_err(|e| {
        eprintln!("[sub-injector] send error for {path}: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let content_type = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if resp.content_length().unwrap_or(0) > MAX_BODY_SIZE {
        eprintln!("[sub-injector] upstream body too large for {path}");
        return Err(StatusCode::BAD_GATEWAY);
    }

    let body_bytes = resp.bytes().await.map_err(|e| {
        eprintln!("[sub-injector] body error for {path}: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    if body_bytes.len() as u64 > MAX_BODY_SIZE {
        eprintln!("[sub-injector] upstream body too large for {path}");
        return Err(StatusCode::BAD_GATEWAY);
    }

    let is_yaml_or_json = content_type.contains("yaml") || content_type.contains("json");
    let matched_rule = find_matching_rule(&headers, &cfg.injections);
    eprintln!(
        "[sub-injector] matched_rule={} ct={content_type:?} body_len={}",
        matched_rule.map(|r| r.links_source.as_str()).unwrap_or("none"),
        body_bytes.len()
    );

    let final_body: Bytes = if let Some(rule) = matched_rule {
        if !is_yaml_or_json {
            let extra = load_extra_links_from_source(&rule.links_source, &cfg.client).await;
            eprintln!("[sub-injector] extra_len={}", extra.len());
            if !extra.is_empty() {
                let injected = inject_links(&body_bytes, &extra);
                eprintln!("[sub-injector] injected body_len={}", injected.len());
                injected.into()
            } else {
                body_bytes
            }
        } else {
            body_bytes
        }
    } else {
        body_bytes
    };

    // Hop-by-hop headers that must not be forwarded; also skip content-length
    // since reqwest decompresses the body (making the original length wrong)
    const SKIP: &[&str] = &[
        "connection", "transfer-encoding", "trailer",
        "upgrade", "keep-alive", "content-length",
    ];

    let mut response = Response::builder().status(status.as_u16());
    for (name, value) in &resp_headers {
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            response = response.header(name.as_str(), v);
        }
    }

    response
        .body(axum::body::Body::from(final_body))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub fn build_app(cfg: Arc<AppState>) -> Router {
    Router::new()
        .route("/{*path}", get(proxy))
        .route("/", get(proxy))
        .with_state(cfg)
}

#[tokio::main]
async fn main() {
    let config_path = env::var("CONFIG_FILE").unwrap_or_else(|_| "config.toml".to_string());
    let file_cfg = load_config(&config_path);
    let bind_addr = file_cfg.bind_addr.unwrap_or_else(|| "0.0.0.0:3020".to_string());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client");

    let cfg = Arc::new(AppState {
        upstream: file_cfg.upstream_url,
        bind_addr: bind_addr.clone(),
        injections: file_cfg.injections,
        client,
    });

    let app = build_app(cfg);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .expect("Failed to bind");

    println!("sub-injector v{} listening on {bind_addr}", env!("CARGO_PKG_VERSION"));
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    const FAKE_LINKS: &str = "vless://aaaaa@1.2.3.4:443#node1\nss://bbbbbb@5.6.7.8:8388#node2";
    const EXTRA_LINKS: &str = "hysteria2://PASS@10.0.0.1:5350?obfs=salamander&obfs-password=OBFS#node-fi\nhysteria2://PASS@10.0.0.2:5350?obfs=salamander&obfs-password=OBFS#node-pl";

    fn fake_b64() -> String {
        STANDARD.encode(FAKE_LINKS.as_bytes())
    }

    fn decode_b64(s: &str) -> String {
        let stripped: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        STANDARD
            .decode(&stripped)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default()
    }

    fn write_temp_file(content: &str) -> String {
        let path = std::env::temp_dir().join(format!("test-extra-links-{}-{}.txt", std::process::id(), rand_suffix()));
        fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos() as u64
    }

    fn hy2_rules(links_source: &str) -> Vec<InjectionRule> {
        vec![InjectionRule {
            header: "User-Agent".to_string(),
            contains: vec![
                "happ".to_string(), "hiddify".to_string(), "nekobox".to_string(),
                "nekoray".to_string(), "sing-box".to_string(), "clash.meta".to_string(),
                "mihomo".to_string(), "v2rayng".to_string(),
            ],
            links_source: links_source.to_string(),
        }]
    }

    fn make_cfg(upstream: String, injections: Vec<InjectionRule>) -> Arc<AppState> {
        Arc::new(AppState {
            upstream,
            bind_addr: "0.0.0.0:3020".to_string(),
            injections,
            client: reqwest::Client::new(),
        })
    }

    async fn start_mock(body: &'static str, content_type: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let app = Router::new().route(
                "/{*path}",
                get(move || async move {
                    Response::builder()
                        .header("content-type", content_type)
                        .body(axum::body::Body::from(body))
                        .unwrap()
                }),
            );
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        format!("http://127.0.0.1:{}", port)
    }

    async fn call(app: Router, path: &str, ua: &str) -> (u16, String) {
        let req = Request::builder()
            .uri(path)
            .header("user-agent", ua)
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    // ── unit tests ─────────────────────────────────────────────────────────────

    #[test]
    fn ua_matching_compatible() {
        let rules = hy2_rules("/tmp/fake.txt");
        let mut headers = HeaderMap::new();
        for ua in &["happ/2.0", "Hiddify/1.5", "NekoBox/3.0", "sing-box/1.0",
                    "clash.meta/1.18", "Mihomo/1.0", "v2rayng/1.8", "NekoRay/3.0"] {
            headers.insert("user-agent", ua.parse().unwrap());
            assert!(find_matching_rule(&headers, &rules).is_some(), "{ua} should match");
        }
    }

    #[test]
    fn ua_matching_incompatible() {
        let rules = hy2_rules("/tmp/fake.txt");
        let mut headers = HeaderMap::new();
        for ua in &["Shadowrocket/1.0", "Surge/5.0", "QuantumultX", "curl/8.0",
                    "Mozilla/5.0", "clash/1.0"] {
            headers.insert("user-agent", ua.parse().unwrap());
            assert!(find_matching_rule(&headers, &rules).is_none(), "{ua} should NOT match");
        }
    }

    #[test]
    fn inject_links_roundtrip() {
        let body = fake_b64().into_bytes();
        let result = inject_links(&body, EXTRA_LINKS);
        let decoded = decode_b64(&String::from_utf8(result).unwrap());
        assert!(decoded.contains("vless://aaaaa"), "original links preserved");
        assert!(decoded.contains("hysteria2://"), "hysteria2 injected");
        assert!(decoded.contains("node-fi"), "first extra link present");
        assert!(decoded.contains("node-pl"), "second extra link present");
    }

    #[test]
    fn inject_links_invalid_b64_passthrough() {
        let body = b"not-base64!!";
        let result = inject_links(body, EXTRA_LINKS);
        assert_eq!(result, body, "invalid b64 should pass through unchanged");
    }

    #[test]
    fn load_config_parses_correctly() {
        let toml_content = r#"
upstream_url = "http://upstream:2096"
bind_addr = "0.0.0.0:3020"

[[injections]]
header = "User-Agent"
contains = ["hiddify", "happ"]
links_source = "/data/hy2.txt"
"#;
        let path = write_temp_file(toml_content);
        let cfg = load_config(&path);
        assert_eq!(cfg.upstream_url, "http://upstream:2096");
        assert_eq!(cfg.bind_addr, Some("0.0.0.0:3020".to_string()));
        assert_eq!(cfg.injections.len(), 1);
        assert_eq!(cfg.injections[0].contains, vec!["hiddify", "happ"]);
        assert_eq!(cfg.injections[0].links_source, "/data/hy2.txt");
    }

    #[tokio::test]
    async fn load_links_from_file() {
        let path = write_temp_file(EXTRA_LINKS);
        let client = reqwest::Client::new();
        let result = load_extra_links_from_source(&path, &client).await;
        assert_eq!(result, EXTRA_LINKS);
    }

    #[tokio::test]
    async fn load_links_missing_file_returns_empty() {
        let client = reqwest::Client::new();
        let result = load_extra_links_from_source("/tmp/definitely-does-not-exist-xyz.txt", &client).await;
        assert!(result.is_empty());
    }

    // ── integration tests ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn compatible_ua_gets_injection() {
        let upstream = start_mock(Box::leak(fake_b64().into_boxed_str()), "text/plain").await;
        let links_file = write_temp_file(EXTRA_LINKS);
        let cfg = make_cfg(upstream, hy2_rules(&links_file));

        for ua in &["happ/2.0", "Hiddify/1.5", "nekobox/3.0", "sing-box/1.0",
                    "clash.meta/1.18", "mihomo/1.0", "v2rayng/1.8"] {
            let (status, body) = call(build_app(cfg.clone()), "/sub/TOKEN", ua).await;
            let decoded = decode_b64(&body);
            assert_eq!(status, 200, "UA: {ua}");
            assert!(decoded.contains("hysteria2://"), "UA {ua} should inject hysteria2");
            assert!(decoded.contains("vless://aaaaa"), "UA {ua} should preserve original links");
        }
    }

    #[tokio::test]
    async fn incompatible_ua_passes_through() {
        let upstream = start_mock(Box::leak(fake_b64().into_boxed_str()), "text/plain").await;
        let links_file = write_temp_file(EXTRA_LINKS);
        let cfg = make_cfg(upstream, hy2_rules(&links_file));

        for ua in &["Shadowrocket/1.0", "Surge/5.0", "QuantumultX", "Mozilla/5.0"] {
            let (status, body) = call(build_app(cfg.clone()), "/sub/TOKEN", ua).await;
            let decoded = decode_b64(&body);
            assert_eq!(status, 200, "UA: {ua}");
            assert!(!decoded.contains("hysteria2://"), "UA {ua} should NOT inject");
            assert!(decoded.contains("vless://aaaaa"), "UA {ua} should preserve original links");
        }
    }

    #[tokio::test]
    async fn yaml_content_type_never_injected() {
        let upstream = start_mock("proxies:\n  - name: node1\n    type: vless", "text/yaml").await;
        let links_file = write_temp_file(EXTRA_LINKS);
        let cfg = make_cfg(upstream, hy2_rules(&links_file));

        let (status, body) = call(build_app(cfg), "/sub/TOKEN", "happ/2.0").await;
        assert_eq!(status, 200);
        assert!(!body.contains("hysteria2://"), "yaml should never be injected");
        assert!(body.contains("proxies:"), "yaml content should be intact");
    }

    #[tokio::test]
    async fn missing_links_file_passthrough() {
        let upstream = start_mock(Box::leak(fake_b64().into_boxed_str()), "text/plain").await;
        let cfg = make_cfg(upstream, hy2_rules("/tmp/nonexistent-links-xyz.txt"));

        let (status, body) = call(build_app(cfg), "/sub/TOKEN", "happ/2.0").await;
        let decoded = decode_b64(&body);
        assert_eq!(status, 200);
        assert!(!decoded.contains("hysteria2://"), "no injection when file missing");
        assert!(decoded.contains("vless://aaaaa"), "original links preserved");
    }

    #[tokio::test]
    async fn upstream_down_returns_502() {
        let cfg = make_cfg(
            "http://127.0.0.1:19999".to_string(),
            hy2_rules("/tmp/any.txt"),
        );
        let (status, _) = call(build_app(cfg), "/sub/TOKEN", "happ/2.0").await;
        assert_eq!(status, 502);
    }

    // ── новые тесты ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn load_links_from_url() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let app = Router::new().route(
                "/links",
                get(|| async { EXTRA_LINKS }),
            );
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{port}/links");
        let result = load_extra_links_from_source(&url, &client).await;
        assert_eq!(result, EXTRA_LINKS);
    }

    #[tokio::test]
    async fn load_links_url_unreachable_returns_empty() {
        let client = reqwest::Client::new();
        let result = load_extra_links_from_source("http://127.0.0.1:19998/links", &client).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn json_content_type_never_injected() {
        let upstream = start_mock(r#"{"proxies":[]}"#, "application/json").await;
        let links_file = write_temp_file(EXTRA_LINKS);
        let cfg = make_cfg(upstream, hy2_rules(&links_file));

        let (status, body) = call(build_app(cfg), "/sub/TOKEN", "happ/2.0").await;
        assert_eq!(status, 200);
        assert!(!body.contains("hysteria2://"), "json should never be injected");
        assert!(body.contains("proxies"), "json content should be intact");
    }

    #[tokio::test]
    async fn custom_header_rule_matches() {
        let upstream = start_mock(Box::leak(fake_b64().into_boxed_str()), "text/plain").await;
        let links_file = write_temp_file(EXTRA_LINKS);
        let cfg = Arc::new(AppState {
            upstream,
            bind_addr: "0.0.0.0:3020".to_string(),
            injections: vec![InjectionRule {
                header: "X-Client-Type".to_string(),
                contains: vec!["premium".to_string()],
                links_source: links_file,
            }],
            client: reqwest::Client::new(),
        });

        // С заголовком — инъекция происходит
        let req = Request::builder()
            .uri("/sub/TOKEN")
            .header("x-client-type", "premium")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = build_app(cfg.clone()).oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let decoded = decode_b64(&String::from_utf8_lossy(&bytes));
        assert!(decoded.contains("hysteria2://"), "premium header should trigger injection");

        // Без заголовка — нет инъекции
        let req = Request::builder()
            .uri("/sub/TOKEN")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = build_app(cfg.clone()).oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let decoded = decode_b64(&String::from_utf8_lossy(&bytes));
        assert!(!decoded.contains("hysteria2://"), "missing header should not trigger injection");
    }

    #[tokio::test]
    async fn no_user_agent_no_match() {
        let upstream = start_mock(Box::leak(fake_b64().into_boxed_str()), "text/plain").await;
        let links_file = write_temp_file(EXTRA_LINKS);
        let cfg = make_cfg(upstream, hy2_rules(&links_file));

        let req = Request::builder()
            .uri("/sub/TOKEN")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = build_app(cfg).oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let decoded = decode_b64(&String::from_utf8_lossy(&bytes));
        assert!(!decoded.contains("hysteria2://"), "no UA should not trigger injection");
        assert!(decoded.contains("vless://aaaaa"), "original links preserved");
    }

    #[test]
    fn inject_links_wrapped_base64() {
        // Некоторые upstream оборачивают base64 по 76 символов
        let raw = STANDARD.encode(FAKE_LINKS.as_bytes());
        let wrapped = raw.as_bytes().chunks(76)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let result = inject_links(wrapped.as_bytes(), EXTRA_LINKS);
        let decoded = decode_b64(&String::from_utf8(result).unwrap());
        assert!(decoded.contains("vless://aaaaa"), "original links preserved after unwrap");
        assert!(decoded.contains("hysteria2://"), "extra links injected after unwrap");
    }

    #[tokio::test]
    async fn upstream_non200_status_proxied() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let app = Router::new().route(
                "/{*path}",
                get(|| async {
                    Response::builder()
                        .status(404)
                        .body(axum::body::Body::from("not found"))
                        .unwrap()
                }),
            );
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        let upstream = format!("http://127.0.0.1:{port}");

        let cfg = make_cfg(upstream, hy2_rules("/tmp/any.txt"));
        let (status, body) = call(build_app(cfg), "/sub/TOKEN", "happ/2.0").await;
        assert_eq!(status, 404);
        assert_eq!(body, "not found");
    }

    #[tokio::test]
    async fn query_string_forwarded() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let app = Router::new().route(
                "/{*path}",
                get(|uri: Uri| async move {
                    let qs = uri.query().unwrap_or("").to_string();
                    Response::builder()
                        .header("content-type", "text/plain")
                        .body(axum::body::Body::from(qs))
                        .unwrap()
                }),
            );
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        let upstream = format!("http://127.0.0.1:{port}");

        let cfg = make_cfg(upstream, vec![]);
        let req = Request::builder()
            .uri("/sub/TOKEN?foo=bar&baz=1")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = build_app(cfg).oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&bytes), "foo=bar&baz=1");
    }

    #[tokio::test]
    async fn hop_by_hop_headers_stripped() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let app = Router::new().route(
                "/{*path}",
                get(|| async {
                    Response::builder()
                        .header("content-type", "text/plain")
                        .header("connection", "keep-alive")
                        .header("x-custom", "preserved")
                        .body(axum::body::Body::from("hello"))
                        .unwrap()
                }),
            );
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        let upstream = format!("http://127.0.0.1:{port}");

        let cfg = make_cfg(upstream, vec![]);
        let req = Request::builder()
            .uri("/sub/TOKEN")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = build_app(cfg).oneshot(req).await.unwrap();
        let headers = resp.headers().clone();
        assert!(headers.get("connection").is_none(), "connection must be stripped");
        assert!(headers.get("x-custom").is_some(), "x-custom must be preserved");
    }

    #[tokio::test]
    async fn different_ua_gets_different_links() {
        let upstream = start_mock(Box::leak(fake_b64().into_boxed_str()), "text/plain").await;
        let hy2_file = write_temp_file("hysteria2://PASS@1.1.1.1:443#hy2-node");
        let clash_file = write_temp_file("vless://CLASH@2.2.2.2:443#clash-node");

        let cfg = Arc::new(AppState {
            upstream,
            bind_addr: "0.0.0.0:3020".to_string(),
            injections: vec![
                InjectionRule {
                    header: "User-Agent".to_string(),
                    contains: vec!["hiddify".to_string()],
                    links_source: hy2_file,
                },
                InjectionRule {
                    header: "User-Agent".to_string(),
                    contains: vec!["mihomo".to_string()],
                    links_source: clash_file,
                },
            ],
            client: reqwest::Client::new(),
        });

        let (_, body) = call(build_app(cfg.clone()), "/sub/TOKEN", "hiddify/2.0").await;
        let decoded = decode_b64(&body);
        assert!(decoded.contains("hy2-node"), "hiddify should get hy2 links");
        assert!(!decoded.contains("clash-node"), "hiddify should NOT get clash links");

        let (_, body) = call(build_app(cfg.clone()), "/sub/TOKEN", "mihomo/1.0").await;
        let decoded = decode_b64(&body);
        assert!(decoded.contains("clash-node"), "mihomo should get clash links");
        assert!(!decoded.contains("hy2-node"), "mihomo should NOT get hy2 links");
    }
}
