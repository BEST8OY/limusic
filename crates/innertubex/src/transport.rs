//! HTTP transport. context/01. Pure — no Tauri/webview/mpv.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use serde::Serialize;
use sha1::{Digest, Sha1};
use tokio::sync::Notify;

use crate::blocklist::BlockList;
use crate::clients::YouTubeClient;
use crate::models::context::Locale;

pub const ORIGIN_WWW: &str = "https://www.youtube.com";
pub const REFERER_WWW: &str = "https://www.youtube.com/";
pub const API_BASE_WWW: &str = "https://www.youtube.com/youtubei/v1/";

pub const ORIGIN_MUSIC: &str = "https://music.youtube.com";
pub const REFERER_MUSIC: &str = "https://music.youtube.com/";
pub const API_BASE_MUSIC: &str = "https://music.youtube.com/youtubei/v1/";

pub const ORIGIN_MWEB: &str = "https://m.youtube.com";
pub const REFERER_MWEB: &str = "https://m.youtube.com/";
pub const API_BASE_MWEB: &str = "https://m.youtube.com/youtubei/v1/";

pub const ORIGIN_STUDIO: &str = "https://studio.youtube.com";
pub const REFERER_STUDIO: &str = "https://studio.youtube.com/";
pub const API_BASE_STUDIO: &str = "https://studio.youtube.com/youtubei/v1/";

// Kept for backward compatibility
pub const BASE_URL: &str = API_BASE_MUSIC;
pub const ORIGIN: &str = ORIGIN_MUSIC;
pub const REFERER: &str = REFERER_MUSIC;
pub const SW_JS_DATA_URL: &str = "https://www.youtube.com/sw.js_data";

#[derive(Debug, Clone, Copy)]
pub struct EndpointRoute {
    pub api_base: &'static str,
    pub origin: &'static str,
    pub referer: &'static str,
}

/// Resolve endpoint URL and origins dynamically matching MetrolistGroup/innertubex.
pub fn resolve_route(endpoint: &str, client: &YouTubeClient) -> EndpointRoute {
    let clean = endpoint.split('?').next().unwrap_or(endpoint).trim_start_matches('/');
    if clean == "player" && client.use_music_player_endpoint {
        EndpointRoute {
            api_base: API_BASE_MUSIC,
            origin: ORIGIN_MUSIC,
            referer: REFERER_MUSIC,
        }
    } else if client.client_name == "WEB_REMIX" {
        EndpointRoute {
            api_base: API_BASE_MUSIC,
            origin: ORIGIN_MUSIC,
            referer: REFERER_MUSIC,
        }
    } else if client.client_name == "WEB_CREATOR" {
        EndpointRoute {
            api_base: API_BASE_STUDIO,
            origin: ORIGIN_STUDIO,
            referer: REFERER_STUDIO,
        }
    } else if client.client_name == "MWEB" {
        EndpointRoute {
            api_base: API_BASE_MWEB,
            origin: ORIGIN_MWEB,
            referer: REFERER_MWEB,
        }
    } else {
        EndpointRoute {
            api_base: API_BASE_WWW,
            origin: ORIGIN_WWW,
            referer: REFERER_WWW,
        }
    }
}

/// Inject PREF cookie with user's locale settings matching innertubex.
pub fn inject_pref_cookie(cookie: Option<&str>, hl: &str, gl: &str) -> String {
    let pref = format!("f1=50000000&hl={hl}&gl={gl}");
    match cookie {
        Some(c) if !c.is_empty() => {
            if c.contains("PREF=") {
                c.to_owned()
            } else {
                format!("{c}; PREF={pref}")
            }
        }
        _ => format!("PREF={pref}"),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("visitorData not found in sw.js_data")]
    VisitorDataNotFound,
    #[error("Your YouTube Music session expired — open the account menu and sign in again.")]
    SessionExpired,
    #[error("This track is already in the playlist.")]
    AlreadyInPlaylist,
    #[error(
        "YouTube Music only allows custom playlist art on accounts with a verified phone number."
    )]
    CoverRefused,
    #[error("{0}")]
    Other(String),
}

/// Session state, set once at startup / login. context/01 §mutable session state.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub locale: Locale,
    pub visitor_data: Option<String>,
    pub data_sync_id: Option<String>,
    pub auth_user: Option<String>,
    /// Full cookie string (Phase 3). Present ⇒ authenticated requests possible.
    pub cookie: Option<String>,
}

impl Session {
    /// Pull the `SAPISID` value out of the cookie string, if present.
    fn sapisid(&self) -> Option<String> {
        self.cookie.as_deref().and_then(cookie_sapisid).map(str::to_owned)
    }
}

/// Extract the `SAPISID` (or its modern `__Secure-3PAPISID` alias) value from a Cookie header
/// string. Public so the login flow (context/15) can validate a pasted cookie before setting it.
pub fn cookie_sapisid(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        matches!(k.trim(), "SAPISID" | "__Secure-3PAPISID").then(|| v.trim())
    })
}

/// Apply `Set-Cookie` response values to a `Cookie` request header, returning the new header only
/// when something actually changed. Existing names keep their position; new ones go on the end.
///
/// No domain/path matching: every response this is fed comes from music.youtube.com and the jar
/// only ever goes back there, so a `Domain=` a browser would reject cannot reach us.
///
/// ponytail: deletions (`NAME=;`) are ignored rather than applied. A cookie Google wants gone is
/// dead server-side anyway, so carrying it costs nothing, while honouring the deletion would let
/// one odd response drop `SAPISID` and silently sign the user out.
pub(crate) fn merge_set_cookie(cookie: &str, set_cookie: &[&str]) -> Option<String> {
    let mut jar: Vec<(String, String)> = cookie
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut changed = false;
    for line in set_cookie {
        let pair = line.split(';').next().unwrap_or_default();
        let Some((name, value)) = pair.split_once('=') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        match jar.iter_mut().find(|(n, _)| n == name) {
            Some(entry) if entry.1 == value => {}
            Some(entry) => {
                entry.1 = value.to_owned();
                changed = true;
            }
            None => {
                jar.push((name.to_owned(), value.to_owned()));
                changed = true;
            }
        }
    }
    changed.then(|| jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
}

/// The transport client. One shared `reqwest::Client`; proxy must be set before the
/// first request or reqwest snapshots it as none (context/12, the App.kt gotcha).
///
/// `session` is behind a shared lock: the app clones `InnerTube` into the orchestrator, and a
/// runtime login (context/15) must be visible through every clone. Reads/writes are quick and
/// never held across an `.await`, so a std `RwLock` is right (no async lock needed).
#[derive(Clone)]
pub struct InnerTube {
    pub(crate) http: reqwest::Client,
    session: Arc<RwLock<Session>>,
    /// "Hide music videos" (off by default): drop non-ATV rows from the surfaces YouTube
    /// generates. Shared like `session` so a settings toggle reaches every clone, and an atomic
    /// rather than part of `Session` because the endpoints read it on every parse.
    hide_videos: Arc<AtomicBool>,
    /// Artists the user blocked: dropped from the surfaces YouTube generates (radio, autoplay,
    /// home, carousels), never from a list the user opened. Shared like `session` so a settings
    /// change reaches every clone. A `RwLock` rather than an atomic because it is a set, and the
    /// same reasoning as `session` applies: reads are quick and never held across an `.await`.
    blocked: Arc<RwLock<BlockList>>,
    /// Pinged whenever a signed-in request comes back rejected (401/403, or a 200 carrying the
    /// logged-out browse state). The app listens and re-mints the cookie; this crate stays pure,
    /// so it only raises the flag. `notify_one` stores a permit, so a single listener that is
    /// busy healing still sees the next rejection.
    session_rejected: Arc<Notify>,
    /// Pinged when a response's `Set-Cookie` actually changed the stored jar, so the app can
    /// write the rotated cookie back to disk. See [`InnerTube::absorb_cookies`].
    cookie_changed: Arc<Notify>,
}

impl InnerTube {
    pub fn new(session: Session, proxy: Option<&str>) -> Result<Self, Error> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .pool_idle_timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(10);
        if let Some(p) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(p)?);
        }
        Ok(InnerTube {
            http: builder.build()?,
            session: Arc::new(RwLock::new(session)),
            hide_videos: Arc::new(AtomicBool::new(false)),
            blocked: Arc::new(RwLock::new(BlockList::default())),
            session_rejected: Arc::new(Notify::new()),
            cookie_changed: Arc::new(Notify::new()),
        })
    }

    /// Access the underlying reqwest client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Get current session auth user ID if present.
    pub fn auth_user(&self) -> Option<String> {
        self.session.read().ok().and_then(|s| s.auth_user.clone())
    }

    /// Signal raised when YouTube rejects the signed-in session. See the field.
    pub fn session_rejected(&self) -> Arc<Notify> {
        self.session_rejected.clone()
    }

    /// Signal raised when the stored cookie jar was updated from a response. See the field.
    pub fn cookie_changed(&self) -> Arc<Notify> {
        self.cookie_changed.clone()
    }

    /// The session no longer authenticates. Both callers (transport 401/403 and the logged-out
    /// browse payload) route through here so the healer hears about either one.
    pub(crate) fn reject_session(&self) -> Error {
        self.session_rejected.notify_one();
        Error::SessionExpired
    }

    /// Merge a response's `Set-Cookie` into the stored jar.
    ///
    /// Google rotates `__Secure-1PSIDTS` / `__Secure-3PSIDTS` on the very requests this client
    /// makes, and invalidates the previous value when it does. Dropping the new one (which is
    /// what a `reqwest` client with no cookie store does) is what killed the login a few hours
    /// into every session — issue #165 / KI-2. Note that `cookie_store(true)` would not help:
    /// reqwest skips its own store whenever a `Cookie` header is already set, and `headers()`
    /// always sets one.
    fn absorb_cookies(&self, headers: &HeaderMap) {
        if headers.get(SET_COOKIE).is_none() {
            return;
        }
        let set_cookie: Vec<&str> =
            headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).collect();
        {
            let mut s = self.session.write().unwrap();
            let Some(merged) = s.cookie.as_deref().and_then(|c| merge_set_cookie(c, &set_cookie))
            else {
                return;
            };
            s.cookie = Some(merged);
        }
        // Names only, never values: this line ends up in the log a user attaches to a bug report,
        // and it is the one piece of evidence that says whether rotation is being kept.
        let names: Vec<&str> = set_cookie
            .iter()
            .filter_map(|line| line.split(';').next()?.split_once('='))
            .map(|(name, _)| name.trim())
            .collect();
        tracing::debug!(cookies = ?names, "kept rotated cookies");
        self.cookie_changed.notify_one();
    }

    /// Turn "hide music videos" on/off (context: the user setting, default off).
    pub fn set_hide_videos(&self, on: bool) {
        self.hide_videos.store(on, Ordering::Relaxed);
    }

    pub(crate) fn hide_videos(&self) -> bool {
        self.hide_videos.load(Ordering::Relaxed)
    }

    /// Replace the blocked-artist list (the app rebuilds it from the stored settings row on every
    /// change; there is no incremental update).
    pub fn set_blocked(&self, list: BlockList) {
        *self.blocked.write().unwrap() = list;
    }

    pub(crate) fn blocked(&self) -> std::sync::RwLockReadGuard<'_, BlockList> {
        self.blocked.read().unwrap()
    }

    // --- session accessors (context/15) -----------------------------------------------------

    /// True when a login cookie is set.
    pub fn is_logged_in(&self) -> bool {
        self.session.read().unwrap().cookie.is_some()
    }

    /// The current visitorData (read fresh per resolve — a login may have refreshed it).
    pub fn visitor_data(&self) -> Option<String> {
        self.session.read().unwrap().visitor_data.clone()
    }

    /// The current cookie header, if logged in (for the stream-validation HEAD request).
    pub fn cookie(&self) -> Option<String> {
        self.session.read().unwrap().cookie.clone()
    }

    pub fn set_cookie(&self, cookie: Option<String>) {
        self.session.write().unwrap().cookie = cookie;
    }

    pub fn set_data_sync_id(&self, id: Option<String>) {
        self.session.write().unwrap().data_sync_id = id;
    }

    pub fn data_sync_id(&self) -> Option<String> {
        self.session.read().unwrap().data_sync_id.clone()
    }

    pub fn set_visitor_data(&self, vd: Option<String>) {
        self.session.write().unwrap().visitor_data = vd;
    }

    /// Build the request `context` for a client from the current session. Crate-internal — the
    /// endpoints facade calls it. Reads and drops the lock synchronously (no `.await` inside).
    pub(crate) fn context_for(&self, client: &YouTubeClient) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        // `onBehalfOfUser` makes Google *require* a credential: with no cookie it turns a request
        // that would have worked anonymously into a hard 401. Only send it when we can authenticate.
        let dsid = s.cookie.as_ref().and(s.data_sync_id.as_deref());
        client.to_context(&s.locale, s.visitor_data.as_deref(), dsid)
    }

    /// Build a one-off authenticated context for identity validation without changing the shared
    /// session seen by concurrent browse/playback requests. The caller commits the id only after
    /// the validation response succeeds.
    pub(crate) fn context_for_identity(
        &self,
        client: &YouTubeClient,
        data_sync_id: &str,
    ) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        let dsid = s.cookie.as_ref().map(|_| data_sync_id);
        client.to_context(&s.locale, s.visitor_data.as_deref(), dsid)
    }

    /// POST a JSON body to an InnerTube endpoint with this client's headers, retrying
    /// transient network errors (3 attempts, 500ms × 2 backoff). context/01 §retry.
    pub async fn post<B: Serialize>(
        &self,
        path: &str,
        client: &YouTubeClient,
        body: &B,
        set_login: bool,
    ) -> Result<serde_json::Value, Error> {
        let route = resolve_route(path, client);
        // `path` may already carry query params (e.g. browse continuations); chain accordingly.
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!("{}{path}{sep}prettyPrint=false", route.api_base);
        let headers = self.headers(client, path, set_login);
        let body = serde_json::to_vec(body)?;

        let mut delay = Duration::from_millis(500);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let res = self
                .http
                .post(&url)
                .headers(headers.clone())
                .body(body.clone())
                .send()
                .await
                .and_then(|r| r.error_for_status());
            match res {
                Ok(resp) => {
                    self.absorb_cookies(resp.headers());
                    return Ok(resp.json().await?);
                }
                // Retry only on connect/timeout (transient), matching Metrolist's IOException filter.
                Err(e) if attempt < 3 && (e.is_timeout() || e.is_connect() || e.is_request()) => {
                    tracing::warn!(attempt, error = %e, "retrying InnerTube POST {path}");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                // Signed in and Google says "no credential" (401) or "not for you" (403): the
                // stored cookie has gone stale. Raw reqwest text here reads as a broken app and
                // hands the user a URL instead of the one thing that fixes it.
                Err(e)
                    if self.is_logged_in() && e.status().is_some_and(|s| s == 401 || s == 403) =>
                {
                    tracing::warn!(status = ?e.status(), "InnerTube {path} rejected the session");
                    return Err(self.reject_session());
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// POST raw bytes to a path on the same origin that is *not* under `/youtubei`, with this
    /// client's headers plus `extra`, and hand back the response headers along with the body.
    pub(crate) async fn post_upload(
        &self,
        path: &str,
        client: &YouTubeClient,
        extra: &[(&'static str, String)],
        body: Vec<u8>,
    ) -> Result<(HeaderMap, Vec<u8>), Error> {
        let mut headers = self.headers(client, path, true);
        for (name, value) in extra {
            if let Ok(v) = HeaderValue::from_str(value) {
                headers.insert(HeaderName::from_static(name), v);
            }
        }
        if let Ok(v) = HeaderValue::from_str(&body.len().to_string()) {
            headers.insert(reqwest::header::CONTENT_LENGTH, v);
        }
        let route = resolve_route(path, client);
        let resp = self
            .http
            .post(format!("{}/{}", route.origin, path.trim_start_matches('/')))
            .headers(headers)
            .body(body)
            .send()
            .await?
            .error_for_status()?;
        let headers = resp.headers().clone();
        self.absorb_cookies(&headers);
        Ok((headers, resp.bytes().await?.to_vec()))
    }

    /// Per-request headers. Matches MetrolistGroup/innertubex ytClient headers.
    pub fn headers(&self, client: &YouTubeClient, path: &str, set_login: bool) -> HeaderMap {
        let route = resolve_route(path, client);
        let request_referer = if client.is_embedded {
            "https://www.reddit.com/"
        } else {
            route.referer
        };

        let mut h = HeaderMap::new();
        let set = |h: &mut HeaderMap, k: &'static str, v: &str| {
            if let Ok(val) = HeaderValue::from_str(v) {
                h.insert(HeaderName::from_static(k), val);
            }
        };
        set(&mut h, "content-type", "application/json");
        set(&mut h, "accept", "application/json");

        let s = self.session.read().unwrap();
        let accept_lang = s.locale.accept_language_header();
        set(&mut h, "accept-language", &accept_lang);
        set(&mut h, "x-goog-api-format-version", "1");
        set(&mut h, "x-youtube-client-name", &client.client_id);
        set(&mut h, "x-youtube-client-version", &client.client_version);
        set(&mut h, "origin", route.origin);
        set(&mut h, "x-origin", route.origin);
        set(&mut h, "referer", request_referer);
        set(&mut h, "user-agent", &client.user_agent);

        if let Some(vd) = &s.visitor_data {
            set(&mut h, "x-goog-visitor-id", vd);
        }

        // SAPISIDHASH cookie auth — only when logged in AND the client supports it (Phase 3).
        if set_login && client.login_supported {
            if let Some(cookie) = &s.cookie {
                let effective_cookie = inject_pref_cookie(Some(cookie), &s.locale.hl, &s.locale.gl);
                set(&mut h, "cookie", &effective_cookie);

                let auth_user = s.auth_user.as_deref().unwrap_or("0");
                set(&mut h, "x-goog-authuser", auth_user);

                if let Some(sapisid) = s.sapisid() {
                    if let Ok(val) = HeaderValue::from_str(&sapisid_hash(&sapisid, route.origin)) {
                        h.insert(HeaderName::from_static("authorization"), val);
                    }
                }
            }
        }
        h
    }

    /// Fetch visitor data using sw.js_data or homepage fallback.
    pub async fn fetch_visitor_data(&self) -> Result<String, Error> {
        self.fetch_fresh_visitor_data().await
    }

    /// Fetch fresh visitor data matching MetrolistGroup/innertubex fetchFreshVisitorData.
    pub async fn fetch_fresh_visitor_data(&self) -> Result<String, Error> {
        // 1. Try sw.js_data from www.youtube.com
        if let Ok(resp) = self
            .http
            .get(SW_JS_DATA_URL)
            .header("user-agent", crate::clients::YouTubeClient::USER_AGENT_WEB)
            .header("accept", "application/json,text/plain,*/*")
            .send()
            .await
        {
            if let Ok(text) = resp.text().await {
                if let Ok(vd) = parse_visitor_data(&text) {
                    self.set_visitor_data(Some(vd.clone()));
                    return Ok(vd);
                }
            }
        }

        // 2. Try homepage fallback from music.youtube.com
        if let Ok(resp) = self
            .http
            .get(ORIGIN_MUSIC)
            .header("user-agent", crate::clients::YouTubeClient::USER_AGENT_WEB)
            .header(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .send()
            .await
        {
            if let Ok(text) = resp.text().await {
                if let Some(vd) = parse_homepage_visitor_data(&text) {
                    self.set_visitor_data(Some(vd.clone()));
                    return Ok(vd);
                }
            }
        }

        Err(Error::VisitorDataNotFound)
    }

    /// Register a play in watch history: GET the response's
    /// `playbackTracking.videostatsPlaybackUrl.baseUrl` with `c`/`cpn`/`ver` (+ `list`/`referrer`
    /// in a playlist) and the authed client headers. context/01 §registerPlayback. Best-effort —
    /// the caller logs-and-ignores errors.
    pub async fn register_playback(
        &self,
        client: &YouTubeClient,
        base_url: &str,
        cpn: &str,
        playlist_id: Option<&str>,
    ) -> Result<(), Error> {
        let url = build_playback_url(base_url, &client.client_name, cpn, playlist_id);
        let headers = self.headers(client, "playback", true);
        let resp = self.http.get(&url).headers(headers).send().await?.error_for_status()?;
        self.absorb_cookies(resp.headers());
        Ok(())
    }

    /// Register watchtime telemetry with YouTube stats service.
    pub async fn register_watchtime(
        &self,
        client: &YouTubeClient,
        base_url: &str,
        cpn: &str,
        playlist_id: Option<&str>,
        state: &str,
    ) -> Result<(), Error> {
        let mut url = build_playback_url(base_url, &client.client_name, cpn, playlist_id);
        url.push_str(&format!("&state={}", urlencoding::encode(state)));
        let headers = self.headers(client, "watchtime", true);
        let resp = self.http.get(&url).headers(headers).send().await?.error_for_status()?;
        self.absorb_cookies(resp.headers());
        Ok(())
    }
}

/// Build the playback-tracking GET URL. context/01 §registerPlayback. Pure — unit-tested. The
/// `base_url` already carries YouTube's own query params, so we chain onto it.
fn build_playback_url(
    base_url: &str,
    client_name: &str,
    cpn: &str,
    playlist_id: Option<&str>,
) -> String {
    let sep = if base_url.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{base_url}{sep}c={}&cpn={}&ver=2",
        urlencoding::encode(client_name),
        urlencoding::encode(cpn),
    );
    if let Some(list) = playlist_id {
        let enc = urlencoding::encode(list);
        url.push_str(&format!("&list={enc}&referrer={enc}"));
    }
    url
}

/// CPN alphabet — 64 URL-safe chars, exactly 6 bits each. context/01.
const CPN_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A fresh 16-char Content Playback Nonce for one playback. context/01 §registerPlayback.
// ponytail: time+counter-seeded xorshift, not crypto-rand — a CPN only needs to be unique per
// playback, not unpredictable; keeps the `rand` crate out of the tree.
pub fn generate_cpn() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let bump = COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut state = (nanos ^ bump).wrapping_add(0x1234_567);
    if state == 0 {
        state = 0xDEAD_BEEF;
    }
    let mut out = String::with_capacity(16);
    for _ in 0..16 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(CPN_CHARS[(state & 63) as usize] as char);
    }
    out
}

/// `Authorization: SAPISIDHASH <epoch>_<sha1(epoch SAPISID origin)>`. context/01.
pub fn sapisid_hash(sapisid: &str, origin: &str) -> String {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("SAPISIDHASH {epoch}_{}", sha1_hex(&format!("{epoch} {sapisid} {origin}")))
}

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parse visitorData from sw.js_data payload (supporting modern array path [0][2][0][0][13] and legacy shapes).
pub fn parse_visitor_data(body: &str) -> Result<String, Error> {
    // Drop everything up to and including the first newline or the `)]}'` guard.
    let json_start = body.find('[').ok_or(Error::VisitorDataNotFound)?;
    let value: serde_json::Value = serde_json::from_str(&body[json_start..])?;

    // Check nested array path [0][2][0][0][13] (modern YouTube sw.js_data)
    if let Some(vd) = value
        .get(0)
        .and_then(|v| v.get(2))
        .and_then(|v| v.get(0))
        .and_then(|v| v.get(0))
        .and_then(|v| v.get(13))
        .and_then(|v| v.as_str())
    {
        if !vd.is_empty() {
            return Ok(vd.to_owned());
        }
    }

    // Check array path [0][2] filter by Cgt/Cgs (innertube legacy)
    if let Some(arr) = value.get(0).and_then(|v| v.get(2)).and_then(|v| v.as_array()) {
        if let Some(s) = arr
            .iter()
            .filter_map(|v| v.as_str())
            .find(|s| s.starts_with("Cgt") || s.starts_with("Cgs"))
        {
            return Ok(s.to_owned());
        }
    }

    Err(Error::VisitorDataNotFound)
}

/// Parse visitorData from YouTube Music homepage HTML via regex.
pub fn parse_homepage_visitor_data(html: &str) -> Option<String> {
    let re = regex::Regex::new(r#"(?:VISITOR_DATA|visitorData)"\s*:\s*"([^"]+)""#).ok()?;
    re.captures(html).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned())
}

/// Validate media URL host and scheme matching MetrolistGroup/innertubex.
pub fn validated_media_url(url_str: &str) -> Result<String, Error> {
    let parsed = reqwest::Url::parse(url_str).map_err(|e| Error::Other(e.to_string()))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(Error::Other("invalid media url scheme".into()));
    }
    let host = parsed.host_str().unwrap_or_default();
    if !host.ends_with(".googlevideo.com")
        && !host.ends_with(".youtube.com")
        && host != "googlevideo.com"
        && host != "youtube.com"
    {
        return Err(Error::Other(format!("untrusted media host: {host}")));
    }
    Ok(url_str.to_owned())
}

/// Validate stats URL host and path matching MetrolistGroup/innertubex.
pub fn validated_stats_url(url_str: &str) -> Result<String, Error> {
    let parsed = reqwest::Url::parse(url_str).map_err(|e| Error::Other(e.to_string()))?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(Error::Other("invalid stats url scheme".into()));
    }
    let host = parsed.host_str().unwrap_or_default();
    let valid_hosts = ["s.youtube.com", "www.youtube.com", "music.youtube.com"];
    if !valid_hosts.contains(&host) {
        return Err(Error::Other(format!("untrusted stats host: {host}")));
    }
    let path = parsed.path();
    if path != "/api/stats/playback" && path != "/api/stats/watchtime" {
        return Err(Error::Other(format!("untrusted stats path: {path}")));
    }
    Ok(url_str.to_owned())
}

/// Validate upload URL host and scheme matching MetrolistGroup/innertubex.
pub fn validated_upload_url(url_str: &str) -> Result<String, Error> {
    let parsed = reqwest::Url::parse(url_str).map_err(|e| Error::Other(e.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(Error::Other("invalid upload url scheme".into()));
    }
    let host = parsed.host_str().unwrap_or_default();
    if !host.ends_with(".youtube.com") && !host.ends_with(".googlevideo.com") {
        return Err(Error::Other(format!("untrusted upload host: {host}")));
    }
    Ok(url_str.to_owned())
}

fn add_query_before_fragment(url: &str, param: &str) -> String {
    let (before, fragment) = match url.find('#') {
        Some(idx) => (&url[..idx], &url[idx..]),
        None => (url, ""),
    };
    let sep = if before.contains('?') { "&" } else { "?" };
    format!("{before}{sep}{param}{fragment}")
}

/// Append client playback nonce (cpn) to media URL matching MetrolistGroup/innertubex.
pub fn append_client_playback_nonce(url: &str, cpn: &str) -> String {
    if cpn.len() != 16 || !cpn.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return url.to_owned();
    }
    if url.contains("cpn=") {
        return url.to_owned();
    }
    add_query_before_fragment(url, &format!("cpn={cpn}"))
}

/// Replace client playback nonce (cpn) in media URL matching MetrolistGroup/innertubex.
pub fn replace_client_playback_nonce(url: &str, cpn: &str) -> String {
    if cpn.len() != 16 || !cpn.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return url.to_owned();
    }
    if let Ok(re) = regex::Regex::new(r"([?&])cpn=[^&#]*") {
        if re.is_match(url) {
            return re.replace(url, format!("${{1}}cpn={cpn}")).to_string();
        }
    }
    append_client_playback_nonce(url, cpn)
}

/// Validate caption URL endpoint matching MetrolistGroup/innertubex.
pub fn validated_caption_url(url_str: &str) -> Result<String, Error> {
    let parsed = reqwest::Url::parse(url_str).map_err(|e| Error::Other(e.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(Error::Other("caption url must use https".into()));
    }
    let host = parsed.host_str().unwrap_or_default();
    let valid_endpoint = match host {
        "youtube.com" | "www.youtube.com" | "music.youtube.com" | "m.youtube.com" => {
            parsed.path() == "/api/timedtext"
        }
        "video.google.com" => parsed.path() == "/timedtext",
        _ => false,
    };
    if !valid_endpoint {
        return Err(Error::Other(format!("untrusted caption host or path: {host}{}", parsed.path())));
    }
    Ok(url_str.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vector() {
        // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        assert_eq!(sha1_hex("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sapisid_hash_shape() {
        let h = sapisid_hash("MYSAPISID", ORIGIN);
        assert!(h.starts_with("SAPISIDHASH "));
        let rest = &h["SAPISIDHASH ".len()..];
        let (epoch, hash) = rest.split_once('_').unwrap();
        assert!(epoch.parse::<u64>().is_ok());
        assert_eq!(hash.len(), 40); // sha1 hex
    }

    #[test]
    fn parse_visitor_data_from_blob() {
        // Shape of sw.js_data: outer array; [0][2] holds the visitorData among other strings.
        let blob = r#")]}'
[["wrs","x",["junk","CgtABCDEFG1234567%3D%3D","more"]],null]"#;
        assert_eq!(parse_visitor_data(blob).unwrap(), "CgtABCDEFG1234567%3D%3D");
    }

    #[test]
    fn playback_url_appends_params() {
        // Base URL already has query params → chained with `&`; playlist adds list+referrer.
        let u = build_playback_url(
            "https://s.youtube.com/api/stats/playback?cl=1&docid=abc",
            "WEB_REMIX",
            "CPN1234567890AB",
            Some("RDAMVMxyz"),
        );
        assert!(u.contains("?cl=1&docid=abc&c=WEB_REMIX&cpn=CPN1234567890AB&ver=2"));
        assert!(u.contains("&list=RDAMVMxyz&referrer=RDAMVMxyz"));
        // No existing query → first param uses `?`, no playlist params.
        let u2 = build_playback_url("https://s.youtube.com/x", "IOS", "abc", None);
        assert_eq!(u2, "https://s.youtube.com/x?c=IOS&cpn=abc&ver=2");
    }

    #[test]
    fn cpn_is_16_url_safe_chars() {
        let cpn = generate_cpn();
        assert_eq!(cpn.len(), 16);
        assert!(cpn.bytes().all(|b| CPN_CHARS.contains(&b)));
        // Two calls in quick succession must differ (counter salt).
        assert_ne!(generate_cpn(), generate_cpn());
    }

    #[test]
    fn on_behalf_of_user_needs_a_cookie() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session { data_sync_id: Some("abc123".into()), ..Default::default() };

        let it = InnerTube::new(session, None).unwrap();
        assert_eq!(it.context_for(web).user.on_behalf_of_user, None, "no cookie ⇒ no obo (401)");

        it.set_cookie(Some("SAPISID=secret".into()));
        assert_eq!(it.context_for(web).user.on_behalf_of_user.as_deref(), Some("abc123"));
    }

    #[test]
    fn identity_validation_context_does_not_mutate_the_committed_session() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session {
            cookie: Some("SAPISID=secret".into()),
            data_sync_id: Some("committed-id".into()),
            ..Default::default()
        };
        let it = InnerTube::new(session, None).unwrap();

        assert_eq!(
            it.context_for_identity(web, "candidate-id").user.on_behalf_of_user.as_deref(),
            Some("candidate-id")
        );
        assert_eq!(it.context_for(web).user.on_behalf_of_user.as_deref(), Some("committed-id"));
    }

    #[test]
    fn sapisid_extracted_from_cookie() {
        let s = Session {
            cookie: Some("FOO=bar; SAPISID=secret123; OTHER=x".into()),
            ..Default::default()
        };
        assert_eq!(s.sapisid().as_deref(), Some("secret123"));
    }

    // The #165 regression: the rotated value has to land in the jar, in place, or the login dies
    // a few hours in.
    #[test]
    fn a_rotated_cookie_replaces_the_stored_one() {
        let merged = merge_set_cookie(
            "SAPISID=keep; __Secure-3PSIDTS=old; PREF=x",
            &["__Secure-3PSIDTS=new; Path=/; Secure; HttpOnly", "YSC=fresh; Path=/"],
        );
        assert_eq!(
            merged.as_deref(),
            Some("SAPISID=keep; __Secure-3PSIDTS=new; PREF=x; YSC=fresh")
        );
    }

    #[test]
    fn nothing_new_means_no_rewrite_and_no_deletions_applied() {
        // Same values back, plus a deletion we deliberately ignore: no change, so no disk write
        // and no chance of dropping the login.
        assert_eq!(
            merge_set_cookie("SAPISID=keep; PREF=x", &["PREF=x; Path=/", "SAPISID=; Max-Age=0"]),
            None
        );
    }
}
