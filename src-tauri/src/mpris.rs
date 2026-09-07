//! Native MPRIS 2.2 D-Bus server for Linux using `zbus 5`.
//!
//! Comprehensive MPRIS 2.2 specification implementation:
//! - Exposes `/org/mpris/MediaPlayer2` implementing `org.mpris.MediaPlayer2` and
//!   `org.mpris.MediaPlayer2.Player`.
//! - Well-known bus name `org.mpris.MediaPlayer2.limusic` (with PID instance fallback).
//! - Rich metadata dictionary (`mpris:trackid`, `mpris:length`, `mpris:artUrl`, `xesam:url`,
//!   `xesam:title`, `xesam:artist`, `xesam:albumArtist`, `xesam:album`, `xesam:trackNumber`,
//!   `xesam:discNumber`, `xesam:asText`).
//! - Full read/write support for `LoopStatus` ("None", "Track", "Playlist"), `Shuffle` (bool),
//!   and `Volume` (0.0–1.0).
//! - Dynamic capabilities (`CanGoNext`, `CanGoPrevious`, `CanPause`, `CanPlay`, `CanSeek`).
//! - Spec-compliant `SetPosition(track_id, position_us)` validating TrackId before seeking.
//! - D-Bus `Seeked(position_us)` signal on scrub.
//! - Root interface window control (`Raise`, `Quit`, `Fullscreen` get/set).
//! - Direct cover art URL (`mpris:artUrl`) support.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, RwLock};
use zbus::fdo;
use zbus::interface;
use zbus::names::WellKnownName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::media::{MediaCapabilities, MediaTrackMetadata};
use crate::state::{AppState, RepeatMode};

const NO_TRACK_PATH: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const MPRIS_OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";


/// Commands sent from the application to the MPRIS event loop.
#[derive(Debug)]
pub enum MprisCommand {
    SetMetadata(Box<MediaTrackMetadata>),
    SetDuration(f64),
    SetPlayback { playing: bool, pos: f64 },
    SetVolume(f64),
    SetRepeat(RepeatMode),
    SetShuffle(bool),
    SetCapabilities(MediaCapabilities),
    Seeked(f64),
}

/// Converts any track ID string (YouTube videoId, local path) into a valid D-Bus ObjectPath.
pub fn format_track_id(id: &str) -> ObjectPath<'static> {
    if id.is_empty() {
        return ObjectPath::from_static_str(NO_TRACK_PATH).unwrap();
    }
    let mut safe = String::with_capacity(id.len() + 24);
    safe.push_str("/org/limusic/track/");
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'_' {
            safe.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(safe, "_{:02x}", byte);
        }
    }
    ObjectPath::try_from(safe)
        .unwrap_or_else(|_| ObjectPath::from_static_str(NO_TRACK_PATH).unwrap())
}

/// Converts RepeatMode to the MPRIS LoopStatus string ("None", "Track", "Playlist").
fn loop_status_to_str(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::Off => "None",
        RepeatMode::One => "Track",
        RepeatMode::All => "Playlist",
    }
}

/// State shared between the D-Bus interface handlers and the update listener.
#[derive(Debug)]
pub struct MprisShared {
    pub current_track_id: ObjectPath<'static>,
    pub metadata: HashMap<String, OwnedValue>,
    pub playback_status: String,
    pub loop_status: String,
    pub shuffle: bool,
    pub volume: f64,
    pub capabilities: MediaCapabilities,
}

fn to_owned_value<T: Into<Value<'static>>>(v: T) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("valid value")
}

impl Default for MprisShared {
    fn default() -> Self {
        let mut metadata = HashMap::new();
        metadata.insert(
            "mpris:trackid".to_string(),
            to_owned_value(Value::ObjectPath(
                ObjectPath::from_static_str(NO_TRACK_PATH).unwrap(),
            )),
        );

        Self {
            current_track_id: ObjectPath::from_static_str(NO_TRACK_PATH).unwrap(),
            metadata,
            playback_status: "Stopped".to_string(),
            loop_status: "None".to_string(),
            shuffle: false,
            volume: 1.0,
            capabilities: MediaCapabilities::default(),
        }
    }
}

/// MPRIS Root interface: `org.mpris.MediaPlayer2`
pub struct MprisRoot {
    app: AppHandle,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        "Limusic"
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "limusic"
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<&str> {
        vec!["file", "http", "https"]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<&str> {
        vec![
            "audio/mpeg",
            "audio/flac",
            "audio/ogg",
            "audio/mp4",
            "audio/opus",
            "audio/webm",
            "audio/aac",
            "audio/x-wav",
            "audio/x-matroska",
        ]
    }

    #[zbus(property)]
    fn can_set_fullscreen(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn fullscreen(&self) -> bool {
        self.app
            .get_webview_window("main")
            .and_then(|w| w.is_fullscreen().ok())
            .unwrap_or(false)
    }

    #[zbus(property)]
    async fn set_fullscreen(&self, fullscreen: bool) {
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.set_fullscreen(fullscreen);
        }
    }

    async fn raise(&self) {
        crate::tray::show_main(&self.app);
    }

    async fn quit(&self) {
        self.app.exit(0);
    }
}

/// MPRIS Player interface: `org.mpris.MediaPlayer2.Player`
pub struct MprisPlayer {
    app: AppHandle,
    shared: Arc<RwLock<MprisShared>>,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    async fn next(&self) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            state.inner().clone().next_in_queue().await;
        }
    }

    async fn previous(&self) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            state.inner().clone().prev_in_queue().await;
        }
    }

    async fn pause(&self) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let _ = state.player.pause();
        }
    }

    async fn play_pause(&self) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            state.inner().clone().resume_or_toggle().await;
        }
    }

    async fn stop(&self) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let _ = state.player.pause();
            let _ = state.user_seek(0.0).await;
        }
    }

    async fn play(&self) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            state.inner().clone().resume_or_toggle().await;
        }
    }

    async fn seek(&self, offset: i64) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let current = state.current_position();
            let target = (current + (offset as f64 / 1_000_000.0)).max(0.0);
            let _ = state.user_seek(target).await;
        }
    }

    async fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        if position < 0 {
            return;
        }
        let current_id = self.shared.read().await.current_track_id.clone();
        // MPRIS specification enforcement: only seek if track_id matches
        if track_id.as_str() != current_id.as_str() {
            return;
        }
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let target = position as f64 / 1_000_000.0;
            let _ = state.user_seek(target).await;
        }
    }

    async fn open_uri(&self, uri: String) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let state = state.inner().clone();
            if let Some(path) = uri.strip_prefix("file://") {
                let item = innertube::SongItem {
                    video_id: format!("{}{path}", crate::local::SONG_PREFIX),
                    title: std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path)
                        .to_string(),
                    ..Default::default()
                };
                state.play_song(item).await;
            }
        }
    }

    #[zbus(signal)]
    pub async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    async fn playback_status(&self) -> String {
        self.shared.read().await.playback_status.clone()
    }

    #[zbus(property)]
    async fn loop_status(&self) -> String {
        self.shared.read().await.loop_status.clone()
    }

    #[zbus(property)]
    async fn set_loop_status(&self, loop_status: &str) {
        let mode = match loop_status {
            "Track" => RepeatMode::One,
            "Playlist" => RepeatMode::All,
            _ => RepeatMode::Off,
        };
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            state.inner().clone().set_repeat(mode).await;
        }
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    async fn shuffle(&self) -> bool {
        self.shared.read().await.shuffle
    }

    #[zbus(property)]
    async fn set_shuffle(&self, shuffle: bool) {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let state = state.inner().clone();
            let is_shuffled = state.is_shuffled().await;
            if is_shuffled != shuffle {
                state.toggle_shuffle().await;
            }
        }
    }

    #[zbus(property)]
    async fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.shared.read().await.metadata.clone()
    }

    #[zbus(property)]
    async fn volume(&self) -> f64 {
        self.shared.read().await.volume
    }

    #[zbus(property)]
    async fn set_volume(&self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        let vol_pct = (volume * 100.0).round() as i64;
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            let state = state.inner().clone();
            let _ = state.player.set_volume(vol_pct);
            let _ = state.app.emit("volume", vol_pct);
        }
    }

    /// Live dynamic query: position is NOT cached and does NOT emit PropertiesChanged.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        if let Some(state) = self.app.try_state::<Arc<AppState>>() {
            (state.current_position() * 1_000_000.0).max(0.0) as i64
        } else {
            0
        }
    }

    #[zbus(property)]
    async fn can_go_next(&self) -> bool {
        self.shared.read().await.capabilities.can_go_next
    }

    #[zbus(property)]
    async fn can_go_previous(&self) -> bool {
        self.shared.read().await.capabilities.can_go_previous
    }

    #[zbus(property)]
    async fn can_play(&self) -> bool {
        self.shared.read().await.capabilities.can_play
    }

    #[zbus(property)]
    async fn can_pause(&self) -> bool {
        self.shared.read().await.capabilities.can_pause
    }

    #[zbus(property)]
    async fn can_seek(&self) -> bool {
        self.shared.read().await.capabilities.can_seek
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// Spawns the MPRIS server on the session bus in a dedicated tokio task.
pub fn spawn_mpris(app: AppHandle, mut rx: mpsc::UnboundedReceiver<MprisCommand>) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_mpris_server(app, &mut rx).await {
            tracing::warn!(error = %e, "MPRIS server failed or exited");
        }
    });
}

async fn run_mpris_server(
    app: AppHandle,
    rx: &mut mpsc::UnboundedReceiver<MprisCommand>,
) -> zbus::Result<()> {
    let shared = Arc::new(RwLock::new(MprisShared::default()));

    let root = MprisRoot { app: app.clone() };
    let player = MprisPlayer {
        app: app.clone(),
        shared: shared.clone(),
    };

    let conn = zbus::connection::Builder::session()?
        .serve_at(MPRIS_OBJECT_PATH, root)?
        .serve_at(MPRIS_OBJECT_PATH, player)?
        .build()
        .await?;

    // Request primary well-known bus name, with PID fallback if collided
    let base_name = "org.mpris.MediaPlayer2.limusic";
    let name = match WellKnownName::try_from(base_name) {
        Ok(wn) => {
            let flags = fdo::RequestNameFlags::ReplaceExisting | fdo::RequestNameFlags::DoNotQueue;
            match conn.request_name_with_flags(&wn, flags).await {
                Ok(_) => base_name.to_string(),
                Err(_) => format!("{base_name}.instance{}", std::process::id()),
            }
        }
        Err(_) => format!("{base_name}.instance{}", std::process::id()),
    };

    if let Ok(wn) = WellKnownName::try_from(name.as_str()) {
        let _ = conn.request_name(&wn).await;
    }

    tracing::info!(name = %name, "Native MPRIS 2.2 service registered");

    // Retrieve interface reference to emit property changed signals
    let iface_ref = conn
        .object_server()
        .interface::<_, MprisPlayer>(MPRIS_OBJECT_PATH)
        .await?;

    let _root_iface_ref = conn
        .object_server()
        .interface::<_, MprisRoot>(MPRIS_OBJECT_PATH)
        .await?;

    // Handle updates from AppState
    while let Some(cmd) = rx.recv().await {
        match cmd {
            MprisCommand::SetMetadata(meta) => {
                let track_id = format_track_id(&meta.track_id);
                let mut map = HashMap::new();

                map.insert(
                    "mpris:trackid".to_string(),
                    to_owned_value(Value::ObjectPath(track_id.clone())),
                );
                if let Some(dur) = meta.duration_secs {
                    let us = (dur * 1_000_000.0).max(0.0) as i64;
                    map.insert("mpris:length".to_string(), to_owned_value(us));
                }

                if !meta.title.is_empty() {
                    map.insert("xesam:title".to_string(), to_owned_value(meta.title));
                }

                if !meta.artists.is_empty() {
                    map.insert("xesam:artist".to_string(), to_owned_value(meta.artists));
                }

                if let Some(al) = meta.album {
                    if !al.is_empty() {
                        map.insert("xesam:album".to_string(), to_owned_value(al));
                    }
                }

                if let Some(aa) = meta.album_artist {
                    if !aa.is_empty() {
                        map.insert("xesam:albumArtist".to_string(), to_owned_value(vec![aa]));
                    }
                }

                if let Some(tn) = meta.track_number {
                    map.insert("xesam:trackNumber".to_string(), to_owned_value(tn));
                }

                if let Some(dn) = meta.disc_number {
                    map.insert("xesam:discNumber".to_string(), to_owned_value(dn));
                }

                if let Some(u) = meta.web_url {
                    map.insert("xesam:url".to_string(), to_owned_value(u));
                }

                if let Some(l) = meta.lyrics {
                    if !l.is_empty() {
                        map.insert("xesam:asText".to_string(), to_owned_value(l));
                    }
                }

                // Handle cover art
                if let Some(ref cover) = meta.cover_url {
                    let file_url = if cover.starts_with('/') {
                        format!("file://{cover}")
                    } else {
                        cover.clone()
                    };
                    map.insert("mpris:artUrl".to_string(), to_owned_value(file_url));
                }

                let is_repeat = {
                    let s = shared.read().await;
                    !s.current_track_id.is_empty() && s.current_track_id == track_id
                };

                {
                    let mut s = shared.write().await;
                    s.current_track_id = track_id;
                    s.metadata = map;
                }

                let player = iface_ref.get().await;
                let emitter = iface_ref.signal_emitter();
                let _ = player.metadata_changed(emitter).await;

                if is_repeat {
                    let _ = MprisPlayer::seeked(emitter, 0).await;
                }
            }
            MprisCommand::SetDuration(secs) => {
                let us = (secs * 1_000_000.0).max(0.0) as i64;
                let should_notify = {
                    let mut s = shared.write().await;
                    let existing = s.metadata.get("mpris:length").and_then(|v| match &**v {
                        Value::I64(val) => Some(*val),
                        _ => None,
                    });
                    s.metadata
                        .insert("mpris:length".to_string(), to_owned_value(us));
                    match existing {
                        None => true,
                        Some(prev_us) => (prev_us - us).abs() > 2_000_000,
                    }
                };
                if should_notify {
                    let player = iface_ref.get().await;
                    let _ = player.metadata_changed(iface_ref.signal_emitter()).await;
                }
            }
            MprisCommand::SetPlayback { playing, pos: _ } => {
                let status = if playing { "Playing" } else { "Paused" };
                let changed = {
                    let mut s = shared.write().await;
                    if s.playback_status != status {
                        s.playback_status = status.to_string();
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    let player = iface_ref.get().await;
                    let _ = player
                        .playback_status_changed(iface_ref.signal_emitter())
                        .await;
                }
            }
            MprisCommand::SetVolume(vol) => {
                let clamped = vol.clamp(0.0, 1.0);
                let changed = {
                    let mut s = shared.write().await;
                    if (s.volume - clamped).abs() > 0.001 {
                        s.volume = clamped;
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    let player = iface_ref.get().await;
                    let _ = player.volume_changed(iface_ref.signal_emitter()).await;
                }
            }
            MprisCommand::SetRepeat(mode) => {
                let loop_str = loop_status_to_str(mode);
                let changed = {
                    let mut s = shared.write().await;
                    if s.loop_status != loop_str {
                        s.loop_status = loop_str.to_string();
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    let player = iface_ref.get().await;
                    let _ = player.loop_status_changed(iface_ref.signal_emitter()).await;
                }
            }
            MprisCommand::SetShuffle(shuffled) => {
                let changed = {
                    let mut s = shared.write().await;
                    if s.shuffle != shuffled {
                        s.shuffle = shuffled;
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    let player = iface_ref.get().await;
                    let _ = player.shuffle_changed(iface_ref.signal_emitter()).await;
                }
            }
            MprisCommand::SetCapabilities(caps) => {
                let old = {
                    let mut s = shared.write().await;
                    let prev = s.capabilities;
                    s.capabilities = caps;
                    prev
                };
                let player = iface_ref.get().await;
                let emitter = iface_ref.signal_emitter();
                if old.can_go_next != caps.can_go_next {
                    let _ = player.can_go_next_changed(emitter).await;
                }
                if old.can_go_previous != caps.can_go_previous {
                    let _ = player.can_go_previous_changed(emitter).await;
                }
                if old.can_play != caps.can_play {
                    let _ = player.can_play_changed(emitter).await;
                }
                if old.can_pause != caps.can_pause {
                    let _ = player.can_pause_changed(emitter).await;
                }
                if old.can_seek != caps.can_seek {
                    let _ = player.can_seek_changed(emitter).await;
                }
            }
            MprisCommand::Seeked(secs) => {
                let us = (secs * 1_000_000.0).max(0.0) as i64;
                let emitter = iface_ref.signal_emitter();
                let _ = MprisPlayer::seeked(emitter, us).await;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Deref;

    #[test]
    fn format_track_id_empty_gives_no_track() {
        let id = format_track_id("");
        assert_eq!(id.as_str(), NO_TRACK_PATH);
    }

    #[test]
    fn format_track_id_sanitizes_complex_identifiers() {
        let simple = format_track_id("dQw4w9WgXcQ");
        assert_eq!(simple.as_str(), "/org/limusic/track/dQw4w9WgXcQ");

        let with_hyphen = format_track_id("-Abc-123_xyz");
        assert_eq!(with_hyphen.as_str(), "/org/limusic/track/_2dAbc_2d123_xyz");

        let local_path = format_track_id("LOCAL:/home/user/Music/song.flac");
        assert_eq!(
            local_path.as_str(),
            "/org/limusic/track/LOCAL_3a_2fhome_2fuser_2fMusic_2fsong_2eflac"
        );

        // Verify that every formatted path parses into a valid ObjectPath
        assert!(ObjectPath::try_from(simple.as_str()).is_ok());
        assert!(ObjectPath::try_from(with_hyphen.as_str()).is_ok());
        assert!(ObjectPath::try_from(local_path.as_str()).is_ok());
    }

    #[test]
    fn loop_status_mapping() {
        assert_eq!(loop_status_to_str(RepeatMode::Off), "None");
        assert_eq!(loop_status_to_str(RepeatMode::One), "Track");
        assert_eq!(loop_status_to_str(RepeatMode::All), "Playlist");
    }

    #[test]
    fn owned_value_helper() {
        let val_i64 = to_owned_value(12345_i64);
        assert_eq!(i64::try_from(&val_i64).unwrap(), 12345);

        let val_str = to_owned_value("hello world".to_string());
        assert_eq!(<&str>::try_from(&val_str).unwrap(), "hello world");

        let val_vec = to_owned_value(vec!["Artist 1".to_string(), "Artist 2".to_string()]);
        let extracted: Vec<&str> = match val_vec.deref() {
            Value::Array(arr) => arr.iter().map(|e| <&str>::try_from(e).unwrap()).collect(),
            _ => panic!("expected array"),
        };
        assert_eq!(extracted, vec!["Artist 1", "Artist 2"]);
    }

    #[test]
    fn repeat_detection_logic() {
        let current_id = format_track_id("track1");
        let same_id = format_track_id("track1");
        let diff_id = format_track_id("track2");

        assert_eq!(current_id, same_id);
        assert_ne!(current_id, diff_id);
    }
}

