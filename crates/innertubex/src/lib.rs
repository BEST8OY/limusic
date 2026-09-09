//! Pure-Rust InnerTube transport + client identities + models + endpoints + rustypipe fallback.
//!
//! The boundary rule (context/11): this crate knows nothing about Tauri, webviews, mpv, or the
//! OS. It is unit-testable against JSON fixtures with no network. Cipher/PoToken/WEB_REMIX
//! streaming are Phase 2 and deliberately absent here.

pub mod blocklist;
pub mod clients;
pub mod endpoints;
pub mod models;
pub mod rustypipe_fallback;
pub mod strategy;
pub mod transport;

pub use blocklist::BlockList;
pub use clients::{
    Clients, PoTokenBinding, YouTubeClient, LYRICS_TIMED_CLIENT, MAIN_CLIENT, METADATA_CLIENT,
    STREAM_FALLBACK_ORDER, UPLOAD_FALLBACK_ORDER,
};
pub use models::browse::{
    AlbumPage, ArtistCarousel, ArtistPage, BrowseItem, HistoryGroup, HomePage,
    PlaylistContinuation, PlaylistPage, PlaylistSort, SearchResults, Section, SortMenu,
};
pub use models::context::Locale;
pub use models::lyrics::{PlainLyrics, TimedLyricLine};
pub use models::metadata::{
    AccountIdentity, AccountInfo, NextResult, Rating, SearchResult, SongItem,
};
pub use models::player::{
    audio_format_score, find_format, find_video_format, select_best_audio_format,
    select_best_video_format, AudioQuality, Format, PlaybackTracking, PlayerResponse,
    StreamingData,
};
pub use rustypipe_fallback::{FallbackError, StreamCandidate};
pub use strategy::{
    AuthenticationPolicy, CapabilitySupport, ClientContentCapabilities, ClientLifecycle,
    ClientSelectionMode, ContentAwareFallbackStrategy, ContentHints, PlaybackClientManifest,
    PlaybackTransport, SelectedClient, MANIFESTS,
};
pub use transport::{
    cookie_sapisid, generate_cpn, inject_pref_cookie, parse_homepage_visitor_data,
    parse_visitor_data, resolve_route, sapisid_hash, validated_media_url, validated_stats_url,
    validated_upload_url, EndpointRoute, Error, InnerTube, Session, API_BASE_MUSIC,
    API_BASE_STUDIO, API_BASE_WWW, BASE_URL, ORIGIN, ORIGIN_MUSIC, ORIGIN_STUDIO, ORIGIN_WWW,
    REFERER, SW_JS_DATA_URL,
};
