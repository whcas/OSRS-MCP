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
//! - `client`: the only network code, [`HardcoreClient`].
//!
//! # Guarantees
//!
//! - Results are always labelled [`WorldType::Hardcore`]. The client targets
//!   the `osrs-hc` endpoint set and never falls back to standard-world data.
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
//! - `serde` and `serde_json`: response parsing. Bodies are parsed by this
//!   crate from already-bounded bytes rather than by `reqwest`.
//! - `tokio` is a dev-dependency only, for the loopback server used in
//!   `tests/market_client.rs`.

mod client;
mod error;
mod model;
mod parse;

pub use client::{ClientConfig, ConfigError, HARDCORE_BASE_URL, HardcoreClient};
pub use error::{MarketDataError, TransportKind};
pub use model::{
    AveragePrice, Endpoint, Freshness, ItemId, ItemInfo, LatestPrices, Lookback, Observed, Price,
    PriceWindow, Provenance, Series, SeriesPoint, SideStats, TimedPrice, UnixSeconds, WindowLength,
    WindowStats, WorldType, classify_age,
};
