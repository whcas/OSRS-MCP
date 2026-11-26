//! HTTP transport for the Hardcore prices API.
//!
//! This is the only place in the crate that performs network I/O for market
//! data. It fetches bytes, bounds them, and hands them to the pure parsers.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::redirect::Policy;
use reqwest::{Client, Url};

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
/// `User-Agent`. Overriding `base_url` exists for tests and mirrors; it does
/// not change which [`WorldType`] results are labelled with, so only point it
/// at Hardcore data.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL ending in the world's path segment, for example
    /// [`HARDCORE_BASE_URL`].
    pub base_url: String,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The base URL did not parse or is not `http` or `https`.
    InvalidBaseUrl,
    /// The `User-Agent` was blank.
    BlankUserAgent,
    /// The underlying HTTP client could not be built.
    HttpClient,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidBaseUrl => "base URL is not a valid http or https URL",
            Self::BlankUserAgent => "user agent must not be blank",
            Self::HttpClient => "HTTP client could not be built",
        })
    }
}

impl std::error::Error for ConfigError {}

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
        let base_url = parse_base_url(&config.base_url)?;
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
        let body = self.get(Endpoint::Mapping, &[]).await?;
        Ok(self.observed(Endpoint::Mapping, parse::mapping(&body)?))
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
        let body = self.get(Endpoint::Latest, &query).await?;
        Ok(self.observed(Endpoint::Latest, parse::latest(&body)?))
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
        let body = self.get(endpoint, &query).await?;
        Ok(self.observed(endpoint, parse::window(endpoint, &body)?))
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
        let body = self.get(Endpoint::Timeseries, &query).await?;
        Ok(self.observed(Endpoint::Timeseries, parse::timeseries(&body)?))
    }

    fn observed<T>(&self, endpoint: Endpoint, value: T) -> Observed<T> {
        let retrieved_at = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Observed {
            value,
            provenance: Provenance {
                world_type: WorldType::Hardcore,
                endpoint,
                retrieved_at: UnixSeconds(retrieved_at),
            },
        }
    }

    /// One bounded GET. Returns the body of a successful response.
    async fn get(
        &self,
        endpoint: Endpoint,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, MarketDataError> {
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
            .get(url)
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
        Ok(body)
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

/// Parses the base URL and guarantees a trailing slash on its path.
fn parse_base_url(text: &str) -> Result<Url, ConfigError> {
    let mut url = Url::parse(text).map_err(|_| ConfigError::InvalidBaseUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(ConfigError::InvalidBaseUrl);
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
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
        let url = parse_base_url(&ClientConfig::default().base_url).unwrap();
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
            Err(ConfigError::InvalidBaseUrl)
        );
        assert_eq!(with("mailto:a@b.c", "ua"), Err(ConfigError::InvalidBaseUrl));
        assert_eq!(
            with(HARDCORE_BASE_URL, "  "),
            Err(ConfigError::BlankUserAgent)
        );
        assert_eq!(with(HARDCORE_BASE_URL, "ua"), Ok(()));
    }
}
