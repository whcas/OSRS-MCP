//! HTTP transport for the Hardcore prices API.
//!
//! This is the only place in the crate that performs network I/O for market
//! data. It fetches bytes, bounds them, and hands them to the pure parsers.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use thiserror::Error;

use crate::domain::{ItemId, UnixSeconds};

use super::error::{MarketDataError, TransportKind};
use super::model::{
    Endpoint, ItemInfo, LatestPrices, Lookback, Observed, PriceWindow, Provenance, Series,
    WindowLength, WorldType,
};
use super::parse;

/// Base URL of the Hardcore Worlds endpoint set. The path segment `osrs-hc`
/// is what selects Hardcore data; the standard-world path is `osrs`.
pub const HARDCORE_BASE_URL: &str = "https://prices.runescape.wiki/api/v2/osrs-hc/";

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest documented response (`/1h`) is under 100 KiB at the time of
/// writing; this leaves generous headroom without allowing unbounded reads.
const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Settings for [`HardcoreClient`].
///
/// The defaults identify this project in the `User-Agent` and use conservative
/// timeouts. The endpoint is not configurable: a client always queries
/// [`HARDCORE_BASE_URL`], so a result labelled Hardcore cannot have come from
/// anywhere else, and [`Provenance::source_url`] records the exact URL anyway.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Value of the `User-Agent` header. Must not be blank. The API operator
    /// asks for a descriptive value and blocks generic client names.
    pub user_agent: String,
    /// Time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Time allowed for the whole request, including reading the body.
    pub request_timeout: Duration,
    /// Largest response body accepted, in bytes.
    pub max_response_bytes: usize,
    /// Source of the current time, replaceable for deterministic tests.
    pub clock: fn() -> SystemTime,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            user_agent: format!(
                "{}/{} (+https://github.com/whcas/OSRS-MCP)",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            ),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            clock: SystemTime::now,
        }
    }
}

/// Why a client could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// The endpoint URLs could not be derived from the base URL. This is not
    /// expected for the built-in Hardcore endpoint.
    #[error("endpoint URLs could not be derived from the base URL")]
    InvalidBaseUrl,
    /// The `User-Agent` was blank.
    #[error("user agent must not be blank")]
    BlankUserAgent,
    /// The underlying HTTP client could not be built.
    #[error("HTTP client could not be built")]
    HttpClient,
}

/// A read-only client for Hardcore Grand Exchange data.
///
/// Every method performs exactly one HTTP GET. There is no retry, polling or
/// caching, and no fallback to standard-world data: failures are returned as
/// [`MarketDataError`]. Callers should fetch bulk endpoints (`latest(None)`,
/// `window`) rather than looping over items, and keep request rates low.
///
/// ```no_run
/// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// use osrs_mcp::domain::ItemId;
/// use osrs_mcp::market::{ClientConfig, HardcoreClient};
///
/// let client = HardcoreClient::new(ClientConfig::default())?;
/// let latest = client.latest(Some(ItemId(4151))).await?;
/// if let Some(prices) = latest.value.get(&ItemId(4151)) {
///     // `None` means this side has never been observed, not zero.
///     println!("last instant-buy: {:?}", prices.high);
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct HardcoreClient {
    http: Client,
    endpoints: EndpointUrls,
    max_response_bytes: usize,
    clock: fn() -> SystemTime,
}

impl HardcoreClient {
    /// Builds a client.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for a blank `User-Agent`, or if the HTTP client
    /// cannot be built.
    pub fn new(config: ClientConfig) -> Result<Self, ConfigError> {
        let base_url = Url::parse(HARDCORE_BASE_URL).map_err(|_| ConfigError::InvalidBaseUrl)?;
        Self::with_base_url(config, &base_url)
    }

    /// The construction path behind [`new`](Self::new), taking the base URL
    /// explicitly. It is private so that no caller outside this module can point
    /// a client at another origin; the unit tests use it to reach a loopback
    /// server. `base` must end with `/` so that endpoint paths append to it.
    fn with_base_url(config: ClientConfig, base: &Url) -> Result<Self, ConfigError> {
        if config.user_agent.trim().is_empty() {
            return Err(ConfigError::BlankUserAgent);
        }
        let endpoints = EndpointUrls::new(base)?;
        let http = Client::builder()
            .user_agent(config.user_agent)
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            // A redirect could land on a different world's data; surface it.
            .redirect(Policy::none())
            .build()
            .map_err(|_| ConfigError::HttpClient)?;
        Ok(Self {
            http,
            endpoints,
            max_response_bytes: config.max_response_bytes,
            clock: config.clock,
        })
    }

    /// Item metadata (`/mapping`) with unique item IDs and names of at most
    /// 256 UTF-8 bytes. Distinct items can share a name.
    ///
    /// # Errors
    ///
    /// Returns [`MarketDataError`] on transport, HTTP, size or parse failure.
    pub async fn mapping(&self) -> Result<Observed<Vec<ItemInfo>>, MarketDataError> {
        let (url, body) = self.get(Endpoint::Mapping, &[]).await?;
        self.observed(Endpoint::Mapping, &url, parse::mapping(&body)?)
    }

    /// Latest observed trades (`/latest`), for one item or, with `None`, all
    /// items. Items the API omits are absent from the result.
    ///
    /// # Errors
    ///
    /// Returns [`MarketDataError`] on transport, HTTP, size or parse failure.
    pub async fn latest(
        &self,
        item: Option<ItemId>,
    ) -> Result<Observed<BTreeMap<ItemId, LatestPrices>>, MarketDataError> {
        let query: Vec<(&str, String)> = item
            .map(|item| ("id", item.0.to_string()))
            .into_iter()
            .collect();
        let (url, body) = self.get(Endpoint::Latest, &query).await?;
        self.observed(Endpoint::Latest, &url, parse::latest(&body, item)?)
    }

    /// Averages and volumes for every item over one window (`/5m` or `/1h`).
    /// With `starts_at` of `None` the API returns its latest completed window.
    ///
    /// # Errors
    ///
    /// Returns [`MarketDataError`] on transport, HTTP, size or parse failure.
    pub async fn window(
        &self,
        length: WindowLength,
        starts_at: Option<UnixSeconds>,
    ) -> Result<Observed<PriceWindow>, MarketDataError> {
        let endpoint = length.endpoint();
        let query: Vec<(&str, String)> = starts_at
            .map(|start| ("timestamp", start.0.to_string()))
            .into_iter()
            .collect();
        let (url, body) = self.get(endpoint, &query).await?;
        self.observed(endpoint, &url, parse::window(endpoint, &body, starts_at)?)
    }

    /// Price history for one item (`/timeseries`).
    ///
    /// # Errors
    ///
    /// Returns [`MarketDataError`] on transport, HTTP, size or parse failure.
    pub async fn timeseries(
        &self,
        item: ItemId,
        lookback: Lookback,
    ) -> Result<Observed<Series>, MarketDataError> {
        let query = [
            ("id", item.0.to_string()),
            ("lookback", lookback.as_query().to_owned()),
        ];
        let (url, body) = self.get(Endpoint::Timeseries, &query).await?;
        self.observed(Endpoint::Timeseries, &url, parse::timeseries(&body, item)?)
    }

    fn observed<T>(
        &self,
        endpoint: Endpoint,
        url: &Url,
        value: T,
    ) -> Result<Observed<T>, MarketDataError> {
        // A clock before the epoch is an error, not a `retrieved_at` of 0 that
        // would read as very stale data.
        let retrieved_at = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| MarketDataError::ClockBeforeEpoch)?
            .as_secs();
        Ok(Observed {
            value,
            provenance: Provenance {
                world_type: WorldType::Hardcore,
                endpoint,
                source_url: url.to_string(),
                retrieved_at: UnixSeconds(retrieved_at),
            },
        })
    }

    /// One bounded GET. Returns the requested URL and the body of a
    /// successful response.
    async fn get(
        &self,
        endpoint: Endpoint,
        query: &[(&str, String)],
    ) -> Result<(Url, Vec<u8>), MarketDataError> {
        let url = self.endpoint_url(endpoint, query);
        let response = self
            .http
            .get(url.clone())
            .send()
            .await
            .map_err(|error| transport_error(endpoint, &error))?;

        let status = response.status();
        if !status.is_success() {
            return Err(MarketDataError::Http {
                endpoint,
                status: status.as_u16(),
            });
        }
        let body = read_bounded_body(response, endpoint, self.max_response_bytes).await?;
        Ok((url, body))
    }

    fn endpoint_url(&self, endpoint: Endpoint, query: &[(&str, String)]) -> Url {
        let mut url = self.endpoints.get(endpoint).clone();
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }
}

/// Reads a response body of at most `limit` bytes. The limit applies twice: to
/// the length the server declares, which is refused before any byte is read,
/// and to the bytes actually received, since the declaration may be absent or
/// wrong.
async fn read_bounded_body(
    mut response: reqwest::Response,
    endpoint: Endpoint,
    limit: usize,
) -> Result<Vec<u8>, MarketDataError> {
    let too_large = MarketDataError::ResponseTooLarge {
        endpoint,
        limit_bytes: limit,
    };
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(too_large);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| transport_error(endpoint, &error))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(too_large);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn transport_error(endpoint: Endpoint, error: &reqwest::Error) -> MarketDataError {
    MarketDataError::Transport {
        endpoint,
        kind: transport_kind(error),
    }
}

/// The URL of every endpoint, joined onto the validated base URL once at
/// construction, so that a request never assembles a URL from strings and a
/// base that cannot take an endpoint path is rejected up front.
#[derive(Debug, Clone)]
struct EndpointUrls {
    mapping: Url,
    latest: Url,
    five_minute: Url,
    one_hour: Url,
    timeseries: Url,
}

impl EndpointUrls {
    fn new(base: &Url) -> Result<Self, ConfigError> {
        let join = |endpoint: Endpoint| {
            base.join(endpoint.path())
                .map_err(|_| ConfigError::InvalidBaseUrl)
        };
        Ok(Self {
            mapping: join(Endpoint::Mapping)?,
            latest: join(Endpoint::Latest)?,
            five_minute: join(Endpoint::FiveMinute)?,
            one_hour: join(Endpoint::OneHour)?,
            timeseries: join(Endpoint::Timeseries)?,
        })
    }

    const fn get(&self, endpoint: Endpoint) -> &Url {
        match endpoint {
            Endpoint::Mapping => &self.mapping,
            Endpoint::Latest => &self.latest,
            Endpoint::FiveMinute => &self.five_minute,
            Endpoint::OneHour => &self.one_hour,
            Endpoint::Timeseries => &self.timeseries,
        }
    }
}

fn transport_kind(error: &reqwest::Error) -> TransportKind {
    if error.is_timeout() {
        TransportKind::Timeout
    } else if error.is_connect() {
        TransportKind::Connect
    } else {
        TransportKind::Other
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    #[test]
    fn a_public_client_always_targets_the_real_hardcore_endpoint() {
        let client = HardcoreClient::new(ClientConfig::default()).unwrap();
        for (endpoint, expected) in [
            (Endpoint::Mapping, "mapping"),
            (Endpoint::Latest, "latest"),
            (Endpoint::FiveMinute, "5m"),
            (Endpoint::OneHour, "1h"),
            (Endpoint::Timeseries, "timeseries"),
        ] {
            assert_eq!(
                client.endpoint_url(endpoint, &[]).as_str(),
                format!("https://prices.runescape.wiki/api/v2/osrs-hc/{expected}"),
            );
        }
    }

    #[test]
    fn default_user_agent_identifies_this_project() {
        let agent = ClientConfig::default().user_agent;
        assert!(agent.starts_with("osrs-mcp/"), "{agent}");
        assert!(agent.contains("github.com/whcas/OSRS-MCP"), "{agent}");
    }

    #[test]
    fn a_blank_user_agent_is_rejected() {
        let with = |user_agent: &str| {
            HardcoreClient::new(ClientConfig {
                user_agent: user_agent.to_owned(),
                ..ClientConfig::default()
            })
            .map(|_| ())
        };
        assert_eq!(with("  "), Err(ConfigError::BlankUserAgent));
        assert_eq!(with(""), Err(ConfigError::BlankUserAgent));
        assert_eq!(with("ua"), Ok(()));
    }

    #[test]
    fn endpoint_urls_append_to_the_base_and_carry_the_query() {
        let base = Url::parse("http://127.0.0.1:9/api/osrs-hc/").unwrap();
        let client = HardcoreClient::with_base_url(ClientConfig::default(), &base).unwrap();
        assert_eq!(
            client
                .endpoint_url(Endpoint::FiveMinute, &[("timestamp", "60".to_owned())])
                .as_str(),
            "http://127.0.0.1:9/api/osrs-hc/5m?timestamp=60"
        );
    }

    #[test]
    fn a_base_that_cannot_take_endpoint_paths_is_rejected() {
        let base = Url::parse("mailto:a@b.c").unwrap();
        assert_eq!(
            HardcoreClient::with_base_url(ClientConfig::default(), &base).map(|_| ()),
            Err(ConfigError::InvalidBaseUrl)
        );
    }

    // The tests below drive the client over HTTP against an in-process server
    // bound to loopback. They are unit tests of this module because they reach
    // the private `with_base_url` constructor; no test touches the network
    // beyond loopback.

    const LATEST: &str = include_str!("../../tests/fixtures/hardcore_ge/latest.json");
    const FIVE_MINUTE: &str = include_str!("../../tests/fixtures/hardcore_ge/five_minute.json");
    const TIMESERIES: &str = include_str!("../../tests/fixtures/hardcore_ge/timeseries_24h.json");
    const MAPPING: &str = include_str!("../../tests/fixtures/hardcore_ge/mapping.json");

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
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let reply = replies[index.min(replies.len() - 1)].clone();
                tokio::spawn(handle_connection(stream, reply, Arc::clone(&log)));
            }
        });
        Mock { base_url, seen }
    }

    /// Serves one connection: records the request head, then sends `reply`. A
    /// connection that closes before sending a full head is dropped unanswered.
    async fn handle_connection(mut stream: TcpStream, reply: Reply, log: Seen) {
        let Some(head) = read_request_head(&mut stream).await else {
            return;
        };
        log.lock().unwrap().push(head);
        let _ = write_reply(&mut stream, reply).await;
    }

    /// Reads up to the blank line that ends an HTTP request head, or `None` if the
    /// connection closes or fails first.
    async fn read_request_head(stream: &mut TcpStream) -> Option<String> {
        let mut head = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => return None,
                Ok(read) => head.extend_from_slice(&buffer[..read]),
            }
        }
        Some(String::from_utf8_lossy(&head).into_owned())
    }

    async fn write_reply(stream: &mut TcpStream, reply: Reply) -> std::io::Result<()> {
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
        client_at(&mock.base_url, adjust)
    }

    fn client_at(base_url: &str, adjust: impl FnOnce(&mut ClientConfig)) -> HardcoreClient {
        let mut config = ClientConfig {
            clock: fixed_now,
            request_timeout: Duration::from_secs(5),
            ..ClientConfig::default()
        };
        adjust(&mut config);
        let base = Url::parse(base_url).unwrap();
        HardcoreClient::with_base_url(config, &base).unwrap()
    }

    #[tokio::test]
    async fn latest_for_one_item_requests_the_hardcore_path_and_labels_provenance() {
        let scoped =
            r#"{"data":{"4151":{"high":8000000,"highTime":1794623592,"low":null,"lowTime":null}}}"#;
        let mock = serve(vec![ok(scoped)]).await;
        let client = client_for(&mock, |_| {});

        let result = client.latest(Some(ItemId(4151))).await.unwrap();

        assert_eq!(
            mock.request_line(0),
            "GET /api/v2/osrs-hc/latest?id=4151 HTTP/1.1"
        );
        assert_eq!(result.provenance.world_type, WorldType::Hardcore);
        assert_eq!(result.provenance.endpoint, Endpoint::Latest);
        assert_eq!(
            result.provenance.source_url,
            format!("{}latest?id=4151", mock.base_url)
        );
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
        // Port 1 on loopback is closed.
        let client = client_at("http://127.0.0.1:1/api/v2/osrs-hc/", |_| {});
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

    #[tokio::test]
    async fn a_response_for_a_different_item_than_requested_is_malformed() {
        // The unscoped fixture holds items 2, 99 and 4151, so it is not a valid
        // answer to a request for item 4151 alone.
        let mock = serve(vec![ok(LATEST)]).await;
        let error = client_for(&mock, |_| {})
            .latest(Some(ItemId(4151)))
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                MarketDataError::Malformed {
                    endpoint: Endpoint::Latest,
                    ..
                }
            ),
            "{error:?}"
        );

        let mock = serve(vec![ok(TIMESERIES)]).await;
        let error = client_for(&mock, |_| {})
            .timeseries(ItemId(2), Lookback::TwentyFourHours)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                MarketDataError::Malformed {
                    endpoint: Endpoint::Timeseries,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_requested_item_missing_from_the_response_is_valid_and_absent() {
        let mock = serve(vec![ok(r#"{"data":{}}"#)]).await;
        let result = client_for(&mock, |_| {})
            .latest(Some(ItemId(4151)))
            .await
            .unwrap();
        assert!(result.value.is_empty());
    }

    fn before_epoch() -> SystemTime {
        UNIX_EPOCH - Duration::from_secs(1)
    }

    #[tokio::test]
    async fn a_clock_before_the_epoch_is_an_error_not_a_zero_retrieval_time() {
        let mock = serve(vec![ok(r#"{"data":{}}"#)]).await;
        let client = client_for(&mock, |config| config.clock = before_epoch);
        assert_eq!(
            client.latest(None).await.unwrap_err(),
            MarketDataError::ClockBeforeEpoch
        );
    }
}
