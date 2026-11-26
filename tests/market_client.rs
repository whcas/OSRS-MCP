//! Behaviour of `HardcoreClient` against a local in-process HTTP server.
//!
//! No test touches the network beyond loopback.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use osrs_mcp::market::{
    ClientConfig, Endpoint, HardcoreClient, ItemId, Lookback, MarketDataError, TransportKind,
    UnixSeconds, WindowLength, WorldType,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const LATEST: &str = include_str!("fixtures/hardcore_ge/latest.json");
const FIVE_MINUTE: &str = include_str!("fixtures/hardcore_ge/five_minute.json");
const TIMESERIES: &str = include_str!("fixtures/hardcore_ge/timeseries_24h.json");
const MAPPING: &str = include_str!("fixtures/hardcore_ge/mapping.json");

/// A frozen clock so `retrieved_at` is deterministic.
fn fixed_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_795_700_000)
}

/// What the mock server does with one connection.
#[derive(Clone)]
enum Reply {
    /// A complete response with the given status line and body.
    Respond { status: &'static str, body: String },
    /// Redirect to another location.
    Redirect,
    /// Wait this long before answering, to provoke a client timeout.
    Stall(Duration),
    /// Send a body with no `Content-Length`, closing the connection to end it.
    Unframed(Vec<u8>),
}

/// Request heads seen by the mock server, in arrival order.
type Seen = Arc<Mutex<Vec<String>>>;

struct Mock {
    base_url: String,
    seen: Seen,
}

impl Mock {
    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn request_line(&self, index: usize) -> String {
        self.requests()[index].lines().next().unwrap().to_owned()
    }

    fn header(&self, index: usize, name: &str) -> Option<String> {
        self.requests()[index].lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    }
}

/// Starts a server that answers each successive connection with the next reply
/// (the last reply repeats).
async fn serve(replies: Vec<Reply>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/api/v2/osrs-hc/", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        for index in 0.. {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let reply = replies[index.min(replies.len() - 1)].clone();
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => head.extend_from_slice(&buffer[..read]),
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&head).into_owned());
                let _ = write_reply(&mut stream, reply).await;
            });
        }
    });
    Mock { base_url, seen }
}

async fn write_reply(stream: &mut tokio::net::TcpStream, reply: Reply) -> std::io::Result<()> {
    match reply {
        Reply::Respond { status, body } => {
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(body.as_bytes()).await?;
        }
        Reply::Redirect => {
            let head = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/api/v2/osrs/latest\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(head.as_bytes()).await?;
        }
        Reply::Stall(delay) => {
            tokio::time::sleep(delay).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await?;
        }
        Reply::Unframed(body) => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .await?;
            stream.write_all(&body).await?;
        }
    }
    stream.shutdown().await
}

fn ok(body: &str) -> Reply {
    Reply::Respond {
        status: "200 OK",
        body: body.to_owned(),
    }
}

fn status(status: &'static str) -> Reply {
    Reply::Respond {
        status,
        body: "{}".to_owned(),
    }
}

fn client_for(mock: &Mock, adjust: impl FnOnce(&mut ClientConfig)) -> HardcoreClient {
    let mut config = ClientConfig {
        base_url: mock.base_url.clone(),
        clock: fixed_now,
        request_timeout: Duration::from_secs(5),
        ..ClientConfig::default()
    };
    adjust(&mut config);
    HardcoreClient::new(config).unwrap()
}

#[tokio::test]
async fn latest_for_one_item_requests_the_hardcore_path_and_labels_provenance() {
    let mock = serve(vec![ok(LATEST)]).await;
    let client = client_for(&mock, |_| {});

    let result = client.latest(Some(ItemId(4151))).await.unwrap();

    assert_eq!(
        mock.request_line(0),
        "GET /api/v2/osrs-hc/latest?id=4151 HTTP/1.1"
    );
    assert_eq!(result.provenance.world_type, WorldType::Hardcore);
    assert_eq!(result.provenance.endpoint, Endpoint::Latest);
    assert_eq!(result.provenance.retrieved_at, UnixSeconds(1_795_700_000));
    let prices = result.value[&ItemId(4151)];
    assert_eq!(prices.high.map(|t| t.price.0), Some(8_000_000));
    assert_eq!(prices.low, None, "a never-seen side stays missing");
}

#[tokio::test]
async fn every_request_carries_the_configured_user_agent() {
    let mock = serve(vec![ok(LATEST)]).await;
    let client = client_for(&mock, |config| {
        config.user_agent = "osrs-mcp-test/9.9 (+https://example.invalid)".to_owned();
    });

    client.latest(None).await.unwrap();

    assert_eq!(mock.request_line(0), "GET /api/v2/osrs-hc/latest HTTP/1.1");
    assert_eq!(
        mock.header(0, "user-agent").as_deref(),
        Some("osrs-mcp-test/9.9 (+https://example.invalid)")
    );
}

#[tokio::test]
async fn window_and_timeseries_and_mapping_use_their_documented_queries() {
    let mock = serve(vec![
        ok(FIVE_MINUTE),
        ok(FIVE_MINUTE),
        ok(TIMESERIES),
        ok(MAPPING),
    ])
    .await;
    let client = client_for(&mock, |_| {});

    let latest_window = client
        .window(WindowLength::FiveMinutes, None)
        .await
        .unwrap();
    assert_eq!(latest_window.value.items.len(), 4);
    assert_eq!(latest_window.provenance.endpoint, Endpoint::FiveMinute);

    client
        .window(WindowLength::FiveMinutes, Some(UnixSeconds(1_795_504_200)))
        .await
        .unwrap();
    let series = client
        .timeseries(ItemId(4151), Lookback::TwentyFourHours)
        .await
        .unwrap();
    assert_eq!(series.value.points.len(), 4);
    let mapping = client.mapping().await.unwrap();
    assert_eq!(mapping.value.len(), 2);

    let lines: Vec<String> = (0..4).map(|index| mock.request_line(index)).collect();
    assert_eq!(
        lines,
        [
            "GET /api/v2/osrs-hc/5m HTTP/1.1",
            "GET /api/v2/osrs-hc/5m?timestamp=1795504200 HTTP/1.1",
            "GET /api/v2/osrs-hc/timeseries?id=4151&lookback=24h HTTP/1.1",
            "GET /api/v2/osrs-hc/mapping HTTP/1.1",
        ]
    );
}

#[tokio::test]
async fn http_errors_carry_the_status_and_are_not_parsed() {
    for (line, code) in [
        ("429 Too Many Requests", 429),
        ("403 Forbidden", 403),
        ("500 Internal Server Error", 500),
        ("404 Not Found", 404),
    ] {
        let mock = serve(vec![status(line)]).await;
        let error = client_for(&mock, |_| {}).latest(None).await.unwrap_err();
        assert_eq!(
            error,
            MarketDataError::Http {
                endpoint: Endpoint::Latest,
                status: code
            }
        );
    }
}

#[tokio::test]
async fn redirects_are_surfaced_not_followed() {
    let mock = serve(vec![Reply::Redirect]).await;
    let error = client_for(&mock, |_| {}).latest(None).await.unwrap_err();
    assert_eq!(
        error,
        MarketDataError::Http {
            endpoint: Endpoint::Latest,
            status: 302
        }
    );
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn invalid_bodies_are_malformed_errors() {
    for body in [
        "",
        "<html>Access denied</html>",
        r#"{"data":[1,2]}"#,
        "null",
    ] {
        let mock = serve(vec![ok(body)]).await;
        let error = client_for(&mock, |_| {}).latest(None).await.unwrap_err();
        assert!(
            matches!(
                error,
                MarketDataError::Malformed {
                    endpoint: Endpoint::Latest,
                    ..
                }
            ),
            "body {body:?} gave {error:?}"
        );
    }
}

#[tokio::test]
async fn partial_data_is_returned_with_missing_sides_intact() {
    let body = r#"{"data":{"1":{"high":10,"highTime":5,"low":null,"lowTime":null},"2":{"high":null,"highTime":null,"low":null,"lowTime":null}}}"#;
    let mock = serve(vec![ok(body)]).await;
    let result = client_for(&mock, |_| {}).latest(None).await.unwrap();
    assert_eq!(result.value.len(), 2);
    assert_eq!(result.value[&ItemId(2)].high, None);
    assert_eq!(result.value[&ItemId(2)].low, None);
}

#[tokio::test]
async fn slow_servers_time_out() {
    let mock = serve(vec![Reply::Stall(Duration::from_secs(5))]).await;
    let client = client_for(&mock, |config| {
        config.request_timeout = Duration::from_millis(100);
    });
    let error = client.latest(None).await.unwrap_err();
    assert_eq!(
        error,
        MarketDataError::Transport {
            endpoint: Endpoint::Latest,
            kind: TransportKind::Timeout
        }
    );
}

#[tokio::test]
async fn connection_failures_are_transport_errors() {
    let mock = serve(vec![ok(LATEST)]).await;
    let client = client_for(&mock, |config| {
        // Port 1 on loopback is closed.
        config.base_url = "http://127.0.0.1:1/api/v2/osrs-hc/".to_owned();
    });
    let error = client.latest(None).await.unwrap_err();
    assert!(
        matches!(
            error,
            MarketDataError::Transport {
                endpoint: Endpoint::Latest,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn bodies_over_the_limit_are_rejected_with_and_without_content_length() {
    let big = format!(r#"{{"data":{{}},"pad":"{}"}}"#, "x".repeat(2_000));
    for reply in [ok(&big), Reply::Unframed(big.clone().into_bytes())] {
        let mock = serve(vec![reply]).await;
        let client = client_for(&mock, |config| config.max_response_bytes = 1_000);
        let error = client.latest(None).await.unwrap_err();
        assert_eq!(
            error,
            MarketDataError::ResponseTooLarge {
                endpoint: Endpoint::Latest,
                limit_bytes: 1_000
            }
        );
    }
}

#[tokio::test]
async fn a_body_exactly_at_the_limit_is_accepted() {
    let body = r#"{"data":{}}"#;
    let mock = serve(vec![ok(body)]).await;
    let client = client_for(&mock, |config| config.max_response_bytes = body.len());
    assert!(client.latest(None).await.unwrap().value.is_empty());
}
