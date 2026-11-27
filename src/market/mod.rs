//! Typed access to Hardcore Grand Exchange price and volume data.
//!
//! Domain-facing types and pure parsing live here, independent of MCP and
//! persistence. See `docs/investigations/hardcore-ge-data-source.md` for the
//! API contract this module implements.
//!
//! # Layout
//!
//! - `model`: typed results with explicit units, provenance and [`classify_age`].
//! - `parse`: pure functions from response bytes to model types.
//! - `search`: pure item lookup over a mapping.
//! - `client`: the only network code, [`HardcoreClient`].
//!
//! # Guarantees
//!
//! - Results are always labelled [`WorldType::Hardcore`]. The client targets
//!   the `osrs-hc` endpoint set and never falls back to standard-world data.
//!   The endpoint is not configurable, so no public caller can point a client
//!   elsewhere; [`Provenance::source_url`] still records the exact URL each
//!   result came from.
//! - Scoped responses are checked against the request: a `latest` or
//!   `timeseries` answer for a different item, a zero timestep or a reversed
//!   range is malformed. Averages are parsed from their exact text and
//!   rejected, not rounded, if they have more than two decimals or use an
//!   exponent.
//! - An observation dated after `now` is [`Freshness::Future`], never fresh.
//! - A price that was never observed is `None`, not zero. Whether an
//!   observation is too old is decided by the caller with [`classify_age`] and
//!   an explicit limit; staleness is not an error.
//! - Failures are [`MarketDataError`] values distinguishing transport, HTTP
//!   status, malformed bodies and oversized responses.
//!
//! # Dependencies
//!
//! - `reqwest` with `default-features = false` and `rustls`: an async HTTPS
//!   client with explicit timeouts and no system TLS dependency. Redirects are
//!   disabled and bodies are read in bounded chunks, so the JSON, query-string
//!   and compression features are not enabled.
//! - `serde` and `serde_json` (with `raw_value`, so average prices are read
//!   from their exact text without a global change to JSON number handling):
//!   response parsing. Bodies are
//!   parsed by this crate from already-bounded bytes rather than by `reqwest`.
//! - `tokio` is a dev-dependency only, for the loopback server used by the
//!   unit tests in `client.rs`.

mod client;
mod error;
mod model;
mod parse;
mod search;

pub use client::{ClientConfig, ConfigError, HARDCORE_BASE_URL, HardcoreClient};
pub use error::{MarketDataError, TransportKind};
pub use model::{
    Endpoint, Freshness, ItemInfo, LatestPrices, Lookback, Observed, PriceWindow, Provenance,
    Series, SeriesPoint, SideStats, TimedPrice, WindowLength, WindowStats, WorldType, classify_age,
};
pub use search::{
    DEFAULT_LIMIT, ItemMatch, ItemQuery, MAX_LIMIT, MAX_NAME_CHARS, MIN_NAME_CHARS, MatchKind,
    NameQuery, QueryError, ResultLimit, SearchOutcome, search,
};
