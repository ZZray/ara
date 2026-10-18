use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Fixture {
    url: Url,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = vec![0; 8192];
                    let Ok(n) = socket.read(&mut request).await else {
                        return;
                    };
                    let text = String::from_utf8_lossy(&request[..n]);
                    let path = text.split_whitespace().nth(1).unwrap_or("/");
                    let (status,kind,extra,body)=match path.split('?').next().unwrap() {
                        "/html" => ("200 OK","text/html","", "<title>Fixture &amp; 标题</title><script>secret_script()</script><style>hide</style><nav>nav secret</nav><main><h1>你好 world</h1><p>First &lt;thing&gt; &#x1F642; &amp; &#20320;</p><a href='/text?api_key=secret&amp;x=ok#frag' data-v='>'>Source</a></main><footer>footer secret</footer>".to_owned()),
                        "/text" => ("200 OK","text/plain","",format!("{}needle ending", "你好界 ".repeat(3000))),
                        "/redirect" => ("302 Found","text/plain","Location: /html\r\n",String::new()),
                        "/private" => ("302 Found","text/plain","Location: http://169.254.169.254/latest/meta-data\r\n",String::new()),
                        "/loop" => ("302 Found","text/plain","Location: /loop\r\n",String::new()),
                        "/large" => ("200 OK","text/plain","", "x".repeat(BODY_LIMIT+1)),
                        "/large-stream" => {
                            let _=socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n").await;
                            let block=vec![b'x';65536];
                            for _ in 0..17 {if socket.write_all(&block).await.is_err(){break;}}
                            return;
                        },
                        "/pdf" => ("200 OK","application/pdf","", "%PDF binary".to_owned()),
                        "/bad-utf8" => { let _=socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\xff\xfe").await; return; },
                        "/slow" => {tokio::time::sleep(Duration::from_secs(5)).await;("200 OK","text/plain","","late".to_owned())},
                        "/search" => ("200 OK","application/json","",r#"{"results":[{"title":"Official","url":"https://example.org/docs","content":"documentation"},{"title":"Duplicate","url":"https://example.org/docs"},{"title":"Unsafe","url":"file:///etc/passwd"},{"title":"Other","url":"https://example.net"}]}"#.to_owned()),
                        "/search-long" => {
                            let mut results=vec![json!({"title":"Redacted source","url":"https://example.org/docs?token=private-value&query=ordinary"})];
                            results.extend([700,2300,3600,3600].iter().enumerate().map(|(i,n)|json!({"title":format!("Long {i}"),"url":long_url(i,*n),"content":"official documentation"})));
                            ("200 OK","application/json","",json!({"results":results}).to_string())
                        },
                        "/links-long" => ("200 OK","text/html","",format!("<p>Official sources</p>{}",(0..4).map(|i|format!("<a href='{}'>Source {i}</a>",long_url(i,3000))).collect::<String>())),
                        "/bad-search" => ("200 OK","application/json","",r#"{"error":"raw private details"}"#.to_owned()),
                        "/challenge" => ("202 Accepted","text/html","","challenge-form".to_owned()),
                        "/challenge-ok" => ("200 OK","text/html","","<title>Please wait</title><form id='challenge-form'>Verify you are human</form>".to_owned()),
                        "/escaped-output" => ("200 OK","text/plain","","\u{1}".repeat(16000)),
                        _ if path.starts_with("/fetch-long/") => ("200 OK","text/plain","","A complete long source URL remains available".to_owned()),
                        _ => ("404 Not Found","text/plain","","missing".to_owned()),
                    };
                    let header=format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",body.len());
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                });
            }
        });
        Self { url: Url::parse(&format!("http://{address}/")).unwrap(), task }
    }
    fn path(&self, path: &str) -> String {
        self.url.join(path).unwrap().to_string()
    }
    fn policy(&self) -> Policy {
        Policy { fixture_origin: Some(self.url.clone()), ..Policy::default() }
    }
}

fn long_url(index: usize, length: usize) -> String {
    format!("https://example.org/source-{index}/{}", "a".repeat(length))
}

#[tokio::test]
async fn actual_search_preserves_long_urls_or_omits_whole_items_and_marks_redaction() {
    let f = Fixture::new().await;
    let result = search(json!({"query":"official sources","max_results":8}), Some(&f.path("search-long")), &f.policy())
        .await
        .unwrap();
    assert!(result.to_string().len() <= 8192);
    assert_eq!(result["truncated"], true);
    let items = result["results"].as_array().unwrap();
    assert_eq!(items[0]["url_redacted"], true);
    assert!(!items[0]["url"].as_str().unwrap().contains("private-value"));
    assert!(result["url_redaction"].as_str().unwrap().contains("cannot reproduce"));
    assert_eq!(items[1]["url"], long_url(0, 700));
    assert_eq!(items[2]["url"], long_url(1, 2300));
    for (i, length) in [700, 2300, 3600, 3600].iter().enumerate() {
        let title = format!("Long {i}");
        if let Some(item) = items.iter().find(|item| item["title"] == title) {
            assert_eq!(item["url"], long_url(i, *length));
            assert_eq!(item["url_redacted"], false);
        }
    }
    assert!(items.len() < 5);
}

#[tokio::test]
async fn actual_fetch_preserves_long_sources_and_omits_links_whole_at_budget() {
    let f = Fixture::new().await;
    let original = f.path(&format!("fetch-long/{}", "a".repeat(2300)));
    let source = fetch(json!({"url":original}), &f.policy()).await.unwrap();
    assert_eq!(source["source"], original);
    assert_eq!(source["final_url"], original);
    assert_eq!(source["source_redacted"], false);
    assert_eq!(source["final_url_redacted"], false);
    let links = fetch(json!({"url":f.path("links-long")}), &f.policy()).await.unwrap();
    assert!(links.to_string().len() <= FETCH_OUTPUT_LIMIT);
    assert_eq!(links["links_truncated"], true);
    let list = links["links"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    for (i, item) in list.iter().enumerate() {
        assert_eq!(item["url"], long_url(i, 3000));
    }
    let secret = fetch(json!({"url":format!("{}?api_key=private-value",f.path("html"))}), &f.policy()).await.unwrap();
    assert_eq!(secret["source_redacted"], true);
    assert_eq!(secret["final_url_redacted"], true);
    assert_eq!(secret["links"][0]["url_redacted"], true);
    assert!(!secret.to_string().contains("private-value"));
}

#[test]
fn tool_contract_has_only_two_read_only_tools() {
    let registry = vec![WebTool::new(false, None), WebTool::new(true, None)];
    assert_eq!(registry.iter().map(|t| t.schema().name.as_str()).collect::<Vec<_>>(), vec!["fetch_url", "web_search"]);
    for tool in registry {
        assert_eq!(tool.schema().parameters.0["additionalProperties"], false);
    }
}
#[test]
fn address_policy_rejects_special_and_mapped_ranges_and_mixed_dns() {
    for address in [
        "0.1.2.3",
        "10.1.2.3",
        "100.64.1.2",
        "127.0.0.1",
        "169.254.169.254",
        "172.31.0.1",
        "192.168.1.2",
        "192.0.0.8",
        "192.0.2.1",
        "198.18.0.1",
        "198.51.100.8",
        "203.0.113.1",
        "224.1.2.3",
        "255.255.255.255",
        "::1",
        "::",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "::ffff:8.8.8.8",
        "2002:808:808::1",
        "2001:db8::1",
        "2001::1",
        "3fff::1",
    ] {
        assert!(!public_ip(address.parse().unwrap()), "{address}");
    }
    for address in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111", "2001:4860:4860::8888"] {
        assert!(public_ip(address.parse().unwrap()), "{address}");
    }
    assert!(validate_addresses(&["8.8.8.8:443".parse().unwrap(), "127.0.0.1:443".parse().unwrap()], false).is_err());
    assert!(validate_addresses(&[], true).is_err());
}
#[test]
fn urls_reject_credentials_and_redact_sensitive_queries() {
    for raw in
        ["file:///tmp/file", "ftp://example.org", "https://user:password@example.org", "https://user@example.org"]
    {
        assert!(valid_url(raw).is_err());
    }
    let clean = safe_url(
        &valid_url(
            "https://example.org/a?api_key=hush&X-Amz-Signature=secret&access_token=token&query=ordinary#private",
        )
        .unwrap(),
    );
    assert!(!clean.contains("hush"));
    assert!(!clean.contains("secret"));
    assert!(!clean.contains("=token"));
    assert!(!clean.contains("private"));
    assert!(clean.contains("ordinary"));
}
#[test]
fn tokenizer_handles_quoted_angles_comments_entities_rawtext_and_lite_results() {
    let base = Url::parse(DEFAULT_SEARCH).unwrap();
    let doc=parse_html("<!-- <a class='result-link' href='evil'>evil</a> --><script>if(a < b) x='<a href=x>'; </script><style>x > y</style><nav>hidden</nav><title>检索</title><a data-v='>' class='result-link' href='//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Fdocs'>官方 &#x4E2D;文</a><td class='result-snippet'>A &amp; B</td><footer>hidden too</footer>",&base);
    assert_eq!(doc.title, "检索");
    assert_eq!(doc.search_links.len(), 1);
    assert_eq!(doc.search_links[0].0, "官方 中文");
    assert_eq!(doc.search_links[0].2, "A & B");
    assert!(!doc.text.contains("hidden"));
    assert!(!doc.text.contains("if(a"));
    assert_eq!(search_target(&doc.search_links[0].1, &base).unwrap().as_str(), "https://example.org/docs");
    assert!(parse_html("<form id='challenge-form'>verify you are human</form>", &base).challenge);
    assert_eq!(decode_entities("&中文中文中文中文中文中文 &#128578; &bad;"), "&中文中文中文中文中文中文 🙂 &bad;");
}

#[tokio::test]
async fn real_http_fetch_extracts_and_tracks_redirect_and_metadata() {
    let f = Fixture::new().await;
    let result = fetch(json!({"url":f.path("redirect")}), &f.policy()).await.unwrap();
    assert!(result["final_url"].as_str().unwrap().ends_with("/html"));
    assert_eq!(result["title"], "Fixture & 标题");
    assert_eq!(result["content_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(result["untrusted"], true);
    let text = result["text"].as_str().unwrap();
    assert!(text.contains("你好 world"));
    assert!(text.contains("🙂"));
    assert!(!text.contains("secret"));
    assert!(!text.contains("hide"));
    assert!(!result["links"][0]["url"].as_str().unwrap().contains("secret"));
}
#[tokio::test]
async fn real_http_pagination_is_utf8_safe_find_is_bounded_and_hash_is_stable() {
    let f = Fixture::new().await;
    let first = fetch(json!({"url":f.path("text"),"max_bytes":257}), &f.policy()).await.unwrap();
    let second =
        fetch(json!({"url":f.path("text"),"max_bytes":257,"offset":first["next_offset"]}), &f.policy()).await.unwrap();
    assert_eq!(first["content_sha256"], second["content_sha256"]);
    assert_eq!(second["offset"], first["next_offset"]);
    assert!(first["text"].as_str().unwrap().len() <= 257);
    let find = fetch(json!({"url":f.path("text"),"find":"needle","max_bytes":256}), &f.policy()).await.unwrap();
    assert!(find["text"].as_str().unwrap().contains("needle"));
    assert!(find["offset"].as_u64().unwrap() > 0);
    assert!(fetch(json!({"url":f.path("text"),"offset":1}), &f.policy()).await.is_err());
    assert!(fetch(json!({"url":f.path("text"),"offset":1,"find":"needle"}), &f.policy()).await.is_err());
}
#[tokio::test]
async fn real_http_size_type_bad_text_redirect_loop_and_private_redirect_are_errors() {
    let f = Fixture::new().await;
    for path in ["large", "large-stream", "pdf", "bad-utf8", "loop", "private", "challenge", "challenge-ok"] {
        assert!(fetch(json!({"url":f.path(path)}), &f.policy()).await.is_err(), "{path}");
    }
    assert!(fetch(json!({"url":f.path("html")}), &Policy::default()).await.is_err());
    assert!(fetch(json!({"url":f.path("escaped-output"),"max_bytes":16000}), &f.policy()).await.is_err());
}

#[test]
fn hostile_bounded_html_and_entity_inputs_do_not_panic_or_expand_unboundedly() {
    let base = Url::parse("https://example.org/").unwrap();
    for input in [
        "&".repeat(BODY_LIMIT),
        "&中文".repeat(BODY_LIMIT / 7),
        "<a data-v='>'>x&amp;y</a>".repeat(BODY_LIMIT / 24),
        "<script>if (a < b) '</scripture>';</script>".repeat(BODY_LIMIT / 41),
    ] {
        let input = clip(&input, BODY_LIMIT);
        let doc = parse_html(&input, &base);
        assert!(doc.text.len() <= BODY_LIMIT);
        assert!(doc.links.len() <= 24);
        assert!(doc.search_links.len() <= 128);
    }
    assert_eq!(decode_entities("&中文中文中文中文中文中文;"), "&中文中文中文中文中文中文;");
}

fn live_receipt(label: &str, result: &Result<AgentToolResult, AgentError>) -> Value {
    match result {
        Err(error) => json!({"tool":label,"status":"error","error":error.to_string()}),
        Ok(result) => {
            let text = result
                .content
                .iter()
                .find_map(|item| match item {
                    rpi_agent::TextContentOrImage::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .expect("tool returned text");
            let value: Value = serde_json::from_str(text).expect("tool returned structured JSON text");
            if label == "web_search" {
                json!({"tool":label,"status":"ok","provider":value["provider"],"results":value["results"].as_array().unwrap().iter().take(2).collect::<Vec<_>>(),"serialized_bytes":text.len(),"untrusted":value["untrusted"]})
            } else {
                json!({"tool":label,"status":"ok","source":value["source"],"final_url":value["final_url"],"title":value["title"],"content_sha256":value["content_sha256"],"total_bytes":value["total_bytes"],"returned_bytes":value["text"].as_str().unwrap().len(),"excerpt":clip(value["text"].as_str().unwrap(),250),"untrusted":value["untrusted"]})
            }
        }
    }
}

#[tokio::test]
#[ignore = "explicit real public-network acceptance; DDG availability is not guaranteed"]
async fn live_public_web_tools_execute_receipt() {
    // Construct directly so a machine's optional private endpoint cannot mask the DDG test.
    let search = WebTool::new(true, None);
    let fetch = WebTool::new(false, None);
    let search_result = search
        .execute(
            "live-search",
            json!({"query":"Python venv official documentation","max_results":3}),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await;
    let fetch_result = fetch
        .execute(
            "live-fetch",
            json!({"url":"https://docs.python.org/3/library/venv.html","max_bytes":1500,"find":"venv"}),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await;
    println!("{}", live_receipt("web_search", &search_result));
    println!("{}", live_receipt("fetch_url", &fetch_result));
    assert!(search_result.is_ok(), "public DDG search did not succeed; see bounded receipt");
    assert!(fetch_result.is_ok(), "official public documentation fetch did not succeed; see bounded receipt");
}
#[tokio::test]
async fn real_http_cancel_and_deadline_cover_request_wait() {
    let f = Fixture::new().await;
    let signal = CancellationToken::new();
    signal.cancel();
    let result = bounded(signal, Duration::from_secs(1), fetch(json!({"url":f.path("slow")}), &f.policy())).await;
    assert!(matches!(result, Err(AgentError::Abort)));
    let signal = CancellationToken::new();
    let cancel = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.cancel();
    });
    let result = bounded(signal, Duration::from_secs(1), fetch(json!({"url":f.path("slow")}), &f.policy())).await;
    assert!(matches!(result, Err(AgentError::Abort)));
    let result =
        bounded(CancellationToken::new(), Duration::from_millis(30), fetch(json!({"url":f.path("slow")}), &f.policy()))
            .await;
    assert!(result.is_err());
}
#[tokio::test]
async fn configured_search_parses_deduplicates_and_rejects_malformed_or_cross_origin_redirect() {
    let f = Fixture::new().await;
    let result =
        search(json!({"query":"official docs","max_results":1}), Some(&f.path("search")), &f.policy()).await.unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["title"], "Official");
    assert_eq!(result["truncated"], true);
    for path in ["bad-search", "private", "challenge"] {
        assert!(search(json!({"query":"x"}), Some(&f.path(path)), &f.policy()).await.is_err());
    }
}
