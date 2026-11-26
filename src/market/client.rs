//! HTTP transport for the Hardcore prices API.
//!
//! This is the only place in the crate that performs network I/O for market
//! data. It fetches bytes, bounds them, and hands them to the pure parsers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use thiserror::Error;

use super::error::{MarketDataError, TransportKind};
use super::model::{
    Endpoint, ItemId, ItemInfo, LatestPrices, Lookback, Observed, PriceWindow, Provenance, Series,
    UnixSeconds, WindowLength, WorldType,
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
/// The defaults target the real Hardcore endpoint with an identifying
/// `User-Agent`. `base_url` is checked at construction: it must be
/// [`HARDCORE_BASE_URL`]. The one exception is a local test server: with
/// `allow_loopback_test_server` set (off by default), an `http` loopback URL
/// whose path ends in `osrs-hc` is also accepted. Any other origin, and the
/// standard-world path, is rejected. Results from a loopback server are still
/// labelled Hardcore, because the label cannot know what a local process
/// serves; [`Provenance::source_url`] shows where each result came from, and
/// production code should never set the flag.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL; see the type documentation for what is accepted.
    pub base_url: String,
    /// Also accept an `http` loopback base URL for a local test server. Off by
    /// default; intended for tests only.
    pub allow_loopback_test_server: bool,
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
            base_url: HARDCORE_BASE_URL.to_owned(),
            allow_loopback_test_server: false,
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
    /// The base URL did not parse.
    #[error("base URL is not a valid URL")]
    InvalidBaseUrl,
    /// The base URL is not the Hardcore endpoint (or, when allowed, a loopback
    /// test server whose path ends in `osrs-hc`).
    #[error("base URL is not the Hardcore endpoint or a loopback osrs-hc test server")]
    NotHardcoreEndpoint,
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
/// use osrs_mcp::market::{ClientConfig, HardcoreClient, ItemId};
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
    base_url: Url,
    max_response_bytes: usize,
    clock: fn() -> SystemTime,
}

impl HardcoreClient {
    /// Builds a client.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for an invalid base URL, a blank `User-Agent`,
    /// or if the HTTP client cannot be built.
    pub fn new(config: ClientConfig) -> Result<Self, ConfigError> {
        if config.user_agent.trim().is_empty() {
            return Err(ConfigError::BlankUserAgent);
        }
        let base_url = parse_base_url(&config.base_url, config.allow_loopback_test_server)?;
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
            base_url,
            max_response_bytes: config.max_response_bytes,
            clock: config.clock,
        })
    }

    /// Item metadata (`/mapping`).
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
    ) -> Result<Observed<std::collections::BTreeMap<ItemId, LatestPrices>>, MarketDataError> {
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
        let transport = |error: &reqwest::Error| MarketDataError::Transport {
            endpoint,
            kind: transport_kind(error),
        };
        let too_large = MarketDataError::ResponseTooLarge {
            endpoint,
            limit_bytes: self.max_response_bytes,
        };

        let url = self.endpoint_url(endpoint, query);
        let mut response = self
            .http
            .get(url.clone())
            .send()
            .await
            .map_err(|error| transport(&error))?;

        let status = response.status();
        if !status.is_success() {
            return Err(MarketDataError::Http {
                endpoint,
                status: status.as_u16(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.max_response_bytes as u64)
        {
            return Err(too_large);
        }

        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| transport(&error))? {
            if body.len().saturating_add(chunk.len()) > self.max_response_bytes {
                return Err(too_large);
            }
            body.extend_from_slice(&chunk);
        }
        Ok((url, body))
    }

    fn endpoint_url(&self, endpoint: Endpoint, query: &[(&str, String)]) -> Url {
        let mut url = self.base_url.clone();
        // The base always ends with a slash (see `parse_base_url`), so appending
        // the segment cannot replace the world's path segment.
        url.set_path(&format!("{}{}", self.base_url.path(), endpoint.path()));
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }
}

/// Parses the base URL, guarantees a trailing slash on its path, and accepts
/// only Hardcore origins (see [`ClientConfig`]).
fn parse_base_url(text: &str, allow_loopback: bool) -> Result<Url, ConfigError> {
    let mut url = Url::parse(text).map_err(|_| ConfigError::InvalidBaseUrl)?;
    if url.cannot_be_a_base() {
        return Err(ConfigError::InvalidBaseUrl);
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    if is_hardcore_origin(&url, allow_loopback) {
        Ok(url)
    } else {
        Err(ConfigError::NotHardcoreEndpoint)
    }
}

fn is_hardcore_origin(url: &Url, allow_loopback: bool) -> bool {
    let plain = url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    let real = url.scheme() == "https"
        && url.host_str() == Some("prices.runescape.wiki")
        && url.port().is_none()
        && url.path() == "/api/v2/osrs-hc/";
    let loopback = allow_loopback
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        && url.path().ends_with("/osrs-hc/");
    plain && (real || loopback)
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
    use super::*;

    #[test]
    fn default_base_url_is_the_hardcore_endpoint_never_standard() {
        let url = parse_base_url(&ClientConfig::default().base_url, false).unwrap();
        assert_eq!(url.path(), "/api/v2/osrs-hc/");
    }

    #[test]
    fn default_user_agent_identifies_this_project() {
        let agent = ClientConfig::default().user_agent;
        assert!(agent.starts_with("osrs-mcp/"), "{agent}");
        assert!(agent.contains("github.com/whcas/OSRS-MCP"), "{agent}");
    }

    #[test]
    fn base_url_gets_a_trailing_slash_and_endpoint_urls_append_to_it() {
        let config = ClientConfig {
            base_url: "http://127.0.0.1:9/api/osrs-hc".to_owned(),
            allow_loopback_test_server: true,
            ..ClientConfig::default()
        };
        let client = HardcoreClient::new(config).unwrap();
        assert_eq!(
            client
                .endpoint_url(Endpoint::FiveMinute, &[("timestamp", "60".to_owned())])
                .as_str(),
            "http://127.0.0.1:9/api/osrs-hc/5m?timestamp=60"
        );
    }

    #[test]
    fn invalid_configuration_is_rejected() {
        let with = |base_url: &str, user_agent: &str| {
            HardcoreClient::new(ClientConfig {
                base_url: base_url.to_owned(),
                user_agent: user_agent.to_owned(),
                ..ClientConfig::default()
            })
            .map(|_| ())
        };
        assert_eq!(with("not a url", "ua"), Err(ConfigError::InvalidBaseUrl));
        assert_eq!(
            with("file:///tmp/x/", "ua"),
            Err(ConfigError::NotHardcoreEndpoint)
        );
        assert_eq!(with("mailto:a@b.c", "ua"), Err(ConfigError::InvalidBaseUrl));
        assert_eq!(
            with(HARDCORE_BASE_URL, "  "),
            Err(ConfigError::BlankUserAgent)
        );
        assert_eq!(with(HARDCORE_BASE_URL, "ua"), Ok(()));
    }

    #[test]
    fn loopback_is_refused_unless_explicitly_allowed() {
        let url = "http://127.0.0.1:8080/api/v2/osrs-hc/";
        assert_eq!(
            parse_base_url(url, false).map(|_| ()),
            Err(ConfigError::NotHardcoreEndpoint)
        );
        assert!(parse_base_url(url, true).is_ok());
        // The flag never widens the real endpoint's checks.
        assert_eq!(
            parse_base_url("https://prices.runescape.wiki/api/v2/osrs/", true).map(|_| ()),
            Err(ConfigError::NotHardcoreEndpoint)
        );
    }

    #[test]
    fn only_hardcore_origins_can_be_labelled_hardcore() {
        let rejected = [
            // Standard-world path on the real host.
            "https://prices.runescape.wiki/api/v2/osrs/",
            "https://prices.runescape.wiki/api/v1/osrs/",
            "https://prices.runescape.wiki/api/v2/osrs-hc/extra/",
            // Right path, wrong host.
            "https://example.com/api/v2/osrs-hc/",
            "https://prices.runescape.wiki.example.com/api/v2/osrs-hc/",
            // Plain http to the real host, credentials, port, query.
            "http://prices.runescape.wiki/api/v2/osrs-hc/",
            "https://user@prices.runescape.wiki/api/v2/osrs-hc/",
            "https://prices.runescape.wiki:8443/api/v2/osrs-hc/",
            "https://prices.runescape.wiki/api/v2/osrs-hc/?x=1",
            // Loopback with a standard-world path, or https loopback.
            "http://127.0.0.1:8080/api/v2/osrs/",
            "http://127.0.0.1:8080/",
            "file:///tmp/osrs-hc/",
        ];
        for base_url in rejected {
            assert_eq!(
                parse_base_url(base_url, true).map(|_| ()),
                Err(ConfigError::NotHardcoreEndpoint),
                "{base_url}"
            );
        }
        for base_url in [
            "https://prices.runescape.wiki/api/v2/osrs-hc",
            "https://prices.runescape.wiki/api/v2/osrs-hc/",
            "http://127.0.0.1:8080/anything/osrs-hc/",
            "http://localhost:3000/osrs-hc",
            "http://[::1]:3000/osrs-hc/",
        ] {
            assert!(parse_base_url(base_url, true).is_ok(), "{base_url}");
        }
    }

    #[test]
    fn the_standard_world_endpoint_cannot_be_used_to_build_a_hardcore_client() {
        for base_url in [
            "https://prices.runescape.wiki/api/v2/osrs/",
            "http://127.0.0.1:8080/api/v2/osrs/",
        ] {
            let config = ClientConfig {
                base_url: base_url.to_owned(),
                allow_loopback_test_server: true,
                ..ClientConfig::default()
            };
            assert_eq!(
                HardcoreClient::new(config).map(|_| ()),
                Err(ConfigError::NotHardcoreEndpoint),
                "{base_url}"
            );
        }
    }

    #[test]
    fn a_loopback_server_is_refused_by_default() {
        let config = ClientConfig {
            base_url: "http://127.0.0.1:8080/api/v2/osrs-hc/".to_owned(),
            ..ClientConfig::default()
        };
        assert_eq!(
            HardcoreClient::new(config).map(|_| ()),
            Err(ConfigError::NotHardcoreEndpoint)
        );
    }
}
