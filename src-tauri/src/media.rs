//! OS media integration (MPRIS / SMTC / NowPlaying).
//!
//! On Linux, Limusic uses a native MPRIS 2.2 D-Bus implementation via `zbus 5` ([`crate::mpris`])
//! offering full MPRIS 2.2 feature parity (rich metadata, full LoopStatus,
//! Shuffle, Volume, dynamic capabilities, trackid verification, and Seeked signal).
//!
//! On Windows and macOS, integration uses `souvlaki` (SMTC on Windows, NowPlaying on macOS).

#[cfg(not(target_os = "linux"))]
use std::sync::Arc;
#[cfg(not(target_os = "linux"))]
use std::time::Duration;

use tauri::AppHandle;
#[cfg(not(target_os = "linux"))]
use tauri::Manager;

#[cfg(not(target_os = "linux"))]
use crate::state::AppState;
use crate::state::RepeatMode;


/// Rich track metadata passed to OS media controls.
#[derive(Debug, Clone, Default)]
pub struct MediaTrackMetadata {
    pub track_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub duration_secs: Option<f64>,
    pub cover_url: Option<String>,
    pub web_url: Option<String>,
    pub lyrics: Option<String>,
}

/// Dynamic playback capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaCapabilities {
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
}

impl Default for MediaCapabilities {
    fn default() -> Self {
        Self {
            can_go_next: false,
            can_go_previous: false,
            can_play: false,
            can_pause: false,
            can_seek: false,
        }
    }
}

/// App-side handle to the media-controls thread/service. Cheap to clone-send into.
#[derive(Clone)]
pub struct MediaHandle {
    #[cfg(target_os = "linux")]
    tx: tokio::sync::mpsc::UnboundedSender<crate::mpris::MprisCommand>,

    #[cfg(not(target_os = "linux"))]
    tx: std::sync::mpsc::Sender<MediaUpdate>,
}

#[cfg(not(target_os = "linux"))]
enum MediaUpdate {
    Metadata {
        title: String,
        artist: String,
        album: Option<String>,
        cover: Option<String>,
    },
    Duration(f64),
    Playback {
        playing: bool,
        pos: f64,
    },
}

impl MediaHandle {
    pub fn set_metadata(&self, meta: MediaTrackMetadata) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::SetMetadata(Box::new(meta)));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let artist = if meta.artists.is_empty() {
                String::new()
            } else {
                meta.artists.join(", ")
            };
            let _ = self.tx.send(MediaUpdate::Metadata {
                title: meta.title,
                artist,
                album: meta.album,
                cover: meta.cover_url,
            });
            if let Some(dur) = meta.duration_secs {
                let _ = self.tx.send(MediaUpdate::Duration(dur));
            }
        }
    }

    pub fn set_duration(&self, secs: f64) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::SetDuration(secs));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = self.tx.send(MediaUpdate::Duration(secs));
        }
    }

    pub fn set_playback(&self, playing: bool, pos: f64) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::SetPlayback { playing, pos });
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = self.tx.send(MediaUpdate::Playback { playing, pos });
        }
    }

    pub fn set_volume(&self, volume_pct: i64) {
        #[cfg(target_os = "linux")]
        {
            let vol = (volume_pct as f64 / 100.0).clamp(0.0, 1.0);
            let _ = self.tx.send(crate::mpris::MprisCommand::SetVolume(vol));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = volume_pct;
        }
    }

    pub fn set_repeat(&self, mode: RepeatMode) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::SetRepeat(mode));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = mode;
        }
    }

    pub fn set_shuffle(&self, shuffled: bool) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::SetShuffle(shuffled));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = shuffled;
        }
    }

    pub fn set_capabilities(&self, caps: MediaCapabilities) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::SetCapabilities(caps));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = caps;
        }
    }

    pub fn notify_seeked(&self, pos_secs: f64) {
        #[cfg(target_os = "linux")]
        {
            let _ = self.tx.send(crate::mpris::MprisCommand::Seeked(pos_secs));
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = pos_secs;
        }
    }
}

/// Spawn the media-controls service or thread.
pub fn spawn(app: AppHandle) -> Option<MediaHandle> {
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        crate::mpris::spawn_mpris(app, rx);
        Some(MediaHandle { tx })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let (tx, rx) = std::sync::mpsc::channel::<MediaUpdate>();
        let spawned = std::thread::Builder::new()
            .name("media-controls".into())
            .spawn(move || run_souvlaki(app, rx));
        match spawned {
            Ok(_) => Some(MediaHandle { tx }),
            Err(e) => {
                tracing::warn!(error = %e, "media-controls thread spawn failed");
                None
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run_souvlaki(app: AppHandle, rx: std::sync::mpsc::Receiver<MediaUpdate>) {
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
        PlatformConfig, SeekDirection,
    };

    #[cfg(target_os = "windows")]
    let hwnd = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as *mut std::ffi::c_void);
    #[cfg(not(target_os = "windows"))]
    let hwnd = None;

    let config = PlatformConfig {
        dbus_name: "limusic",
        display_name: "Limusic",
        hwnd,
    };
    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = ?e, "OS media controls unavailable — skipping");
            return;
        }
    };
    let cb_app = app.clone();
    if let Err(e) = controls.attach(move |event| handle_souvlaki_event(&cb_app, event)) {
        tracing::warn!(error = ?e, "media controls attach failed");
        return;
    }
    tracing::info!("OS media controls attached");

    let mut title = String::new();
    let mut artist = String::new();
    let mut album: Option<String> = None;
    let mut cover: Option<String> = None;
    let mut duration: Option<f64> = None;

    while let Ok(update) = rx.recv() {
        match update {
            MediaUpdate::Metadata {
                title: t,
                artist: a,
                album: al,
                cover: c,
            } => {
                title = t;
                artist = a;
                album = al;
                cover = c;
                duration = None;
                let _ = controls.set_metadata(MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artist),
                    album: album.as_deref(),
                    cover_url: cover.as_deref(),
                    duration: duration.map(Duration::from_secs_f64),
                });
            }
            MediaUpdate::Duration(secs) => {
                duration = Some(secs);
                let _ = controls.set_metadata(MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artist),
                    album: album.as_deref(),
                    cover_url: cover.as_deref(),
                    duration: duration.map(Duration::from_secs_f64),
                });
            }
            MediaUpdate::Playback { playing, pos } => {
                let progress = Some(MediaPosition(Duration::from_secs_f64(pos.max(0.0))));
                let state = if playing {
                    MediaPlayback::Playing { progress }
                } else {
                    MediaPlayback::Paused { progress }
                };
                let _ = controls.set_playback(state);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn handle_souvlaki_event(app: &AppHandle, event: souvlaki::MediaControlEvent) {
    use souvlaki::{MediaControlEvent, MediaPosition, SeekDirection};
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<Arc<AppState>>() else {
            return;
        };
        let state = state.inner().clone();
        match event {
            MediaControlEvent::Play | MediaControlEvent::Toggle => state.resume_or_toggle().await,
            MediaControlEvent::Pause | MediaControlEvent::Stop => {
                let _ = state.player.pause();
            }
            MediaControlEvent::Raise => crate::tray::show_main(&app),
            MediaControlEvent::Next => state.next_in_queue().await,
            MediaControlEvent::Previous => state.prev_in_queue().await,
            MediaControlEvent::SetPosition(MediaPosition(pos)) => {
                let _ = state.player.seek(pos.as_secs_f64());
            }
            MediaControlEvent::SeekBy(dir, by) => {
                let delta = if matches!(dir, SeekDirection::Forward) {
                    by.as_secs_f64()
                } else {
                    -by.as_secs_f64()
                };
                let _ = state.player.seek((state.current_position() + delta).max(0.0));
            }
            MediaControlEvent::Seek(dir) => {
                let delta = if matches!(dir, SeekDirection::Forward) {
                    10.0
                } else {
                    -10.0
                };
                let _ = state.player.seek((state.current_position() + delta).max(0.0));
            }
            _ => {}
        }
    });
}
