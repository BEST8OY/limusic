//! Playback client catalog and content-aware fallback strategy.
//! Port of MetrolistGroup/innertubex `PlaybackClientCatalog` and `ContentAwareFallbackStrategy`.

use std::collections::HashSet;

use crate::clients::{Clients, YouTubeClient};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaybackTransport {
    Direct,
    Hls,
    Sabr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientLifecycle {
    Stable,
    Canary,
    Experimental,
    Deprecated,
    Unreleased,
    Broken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSelectionMode {
    Automatic,
    ProbeOnly,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationPolicy {
    Required,
    Optional,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySupport {
    Supported,
    Limited,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientContentCapabilities {
    pub normal: CapabilitySupport,
    pub explicit: CapabilitySupport,
    pub kids: CapabilitySupport,
    pub age_restricted: CapabilitySupport,
    pub live: CapabilitySupport,
    pub uploads: CapabilitySupport,
}

impl Default for ClientContentCapabilities {
    fn default() -> Self {
        Self {
            normal: CapabilitySupport::Supported,
            explicit: CapabilitySupport::Supported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Limited,
            live: CapabilitySupport::Supported,
            uploads: CapabilitySupport::Supported,
        }
    }
}

pub const fn normal_songs_only_content() -> ClientContentCapabilities {
    ClientContentCapabilities {
        normal: CapabilitySupport::Supported,
        explicit: CapabilitySupport::Unsupported,
        kids: CapabilitySupport::Unsupported,
        age_restricted: CapabilitySupport::Unsupported,
        live: CapabilitySupport::Unsupported,
        uploads: CapabilitySupport::Unsupported,
    }
}

pub const fn vr_content() -> ClientContentCapabilities {
    ClientContentCapabilities {
        normal: CapabilitySupport::Supported,
        explicit: CapabilitySupport::Unsupported,
        kids: CapabilitySupport::Unsupported,
        age_restricted: CapabilitySupport::Unsupported,
        live: CapabilitySupport::Unsupported,
        uploads: CapabilitySupport::Unsupported,
    }
}

#[derive(Debug, Clone)]
pub struct PlaybackClientManifest {
    pub id: &'static str,
    pub client_key: &'static str,
    pub display_name: &'static str,
    pub lifecycle: ClientLifecycle,
    pub selection_mode: ClientSelectionMode,
    pub priority: i32,
    pub authentication: AuthenticationPolicy,
    pub transports: &'static [PlaybackTransport],
    pub content: ClientContentCapabilities,
    pub notes: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct ContentHints {
    pub want_video: bool,
    pub is_explicit: Option<bool>,
    pub is_kids_content: Option<bool>,
    pub is_age_restricted: Option<bool>,
    pub is_uploaded: Option<bool>,
    pub is_live: Option<bool>,
    pub playback_client_override_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFailureKind {
    PlayerRequest,
    Playability,
    Token,
    MediaForbidden,
    MediaFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientHealthContent {
    Normal,
    Explicit,
    Kids,
    AgeRestricted,
    Live,
    Upload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHealthScope {
    pub content: ClientHealthContent,
    pub authenticated: bool,
    pub want_video: bool,
}

impl ClientHealthScope {
    pub fn from_hints(hints: &ContentHints, authenticated: bool) -> Self {
        let content = if hints.is_uploaded == Some(true) {
            ClientHealthContent::Upload
        } else if hints.is_live == Some(true) {
            ClientHealthContent::Live
        } else if hints.is_age_restricted == Some(true) {
            ClientHealthContent::AgeRestricted
        } else if hints.is_kids_content == Some(true) {
            ClientHealthContent::Kids
        } else if hints.is_explicit == Some(true) {
            ClientHealthContent::Explicit
        } else {
            ClientHealthContent::Normal
        };
        Self {
            content,
            authenticated,
            want_video: hints.want_video,
        }
    }
}

pub trait ClientHealthMonitor: Send + Sync {
    fn score_adjustment(&self, _client_id: &str, _scope: Option<&ClientHealthScope>) -> i32 {
        0
    }
    fn record_success(&self, _client_id: &str, _scope: Option<&ClientHealthScope>) {}
    fn record_failure(&self, _client_id: &str, _kind: ClientFailureKind, _scope: Option<&ClientHealthScope>) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackTransportPreference {
    #[default]
    Auto,
    Direct,
    Sabr,
    Hls,
}

#[derive(Debug, Clone, Default)]
pub struct ClientSelectionRequest {
    pub hints: ContentHints,
    pub authenticated: bool,
    pub premium: bool,
    pub fast_path_only: bool,
    pub transport_preference: PlaybackTransportPreference,
    pub excluded_clients: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct RejectedClient {
    pub manifest: &'static PlaybackClientManifest,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClientSelectionResult<'a> {
    pub candidates: Vec<SelectedClient<'a>>,
    pub rejected: Vec<RejectedClient>,
}

#[derive(Debug, Clone)]
pub struct SelectedClient<'a> {
    pub client: &'a YouTubeClient,
    pub manifest: &'static PlaybackClientManifest,
    pub score: i32,
    pub reasons: Vec<String>,
}

pub static MANIFESTS: &[PlaybackClientManifest] = &[
    PlaybackClientManifest {
        id: "VISIONOS_0_1",
        client_key: "VISIONOS_0_1",
        display_name: "visionOS 0.1",
        lifecycle: ClientLifecycle::Experimental,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 100,
        authentication: AuthenticationPolicy::Unsupported,
        transports: &[PlaybackTransport::Direct],
        content: normal_songs_only_content(),
        notes: "Anonymous playback passed normal samples with direct audio streaming.",
    },
    PlaybackClientManifest {
        id: "VISIONOS",
        client_key: "VISIONOS",
        display_name: "visionOS 1.02",
        lifecycle: ClientLifecycle::Experimental,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 95,
        authentication: AuthenticationPolicy::Unsupported,
        transports: &[PlaybackTransport::Direct],
        content: normal_songs_only_content(),
        notes: "Primary direct stream candidate for normal songs.",
    },
    PlaybackClientManifest {
        id: "ANDROID_VR_1_65_10",
        client_key: "ANDROID_VR_1_65_10",
        display_name: "Android VR 1.65",
        lifecycle: ClientLifecycle::Stable,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 85,
        authentication: AuthenticationPolicy::Unsupported,
        transports: &[PlaybackTransport::Direct],
        content: vr_content(),
        notes: "Reliable fallback direct audio candidate.",
    },
    PlaybackClientManifest {
        id: "ANDROID_VR_1_43_32",
        client_key: "ANDROID_VR_1_43_32",
        display_name: "Android VR 1.43",
        lifecycle: ClientLifecycle::Deprecated,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 80,
        authentication: AuthenticationPolicy::Unsupported,
        transports: &[PlaybackTransport::Direct],
        content: vr_content(),
        notes: "Legacy Android VR direct candidate.",
    },
    PlaybackClientManifest {
        id: "WEB_REMIX",
        client_key: "WEB_REMIX",
        display_name: "Web Remix",
        lifecycle: ClientLifecycle::Stable,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 90,
        authentication: AuthenticationPolicy::Optional,
        transports: &[PlaybackTransport::Direct],
        content: ClientContentCapabilities {
            normal: CapabilitySupport::Supported,
            explicit: CapabilitySupport::Supported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Limited,
            live: CapabilitySupport::Supported,
            uploads: CapabilitySupport::Supported,
        },
        notes: "Primary YouTube Music client; supports authenticated high-quality playback and user uploads.",
    },
    PlaybackClientManifest {
        id: "TVHTML5",
        client_key: "TVHTML5",
        display_name: "TV HTML5",
        lifecycle: ClientLifecycle::Stable,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 75,
        authentication: AuthenticationPolicy::Optional,
        transports: &[PlaybackTransport::Direct, PlaybackTransport::Hls],
        content: ClientContentCapabilities {
            normal: CapabilitySupport::Supported,
            explicit: CapabilitySupport::Supported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Limited,
            live: CapabilitySupport::Supported,
            uploads: CapabilitySupport::Supported,
        },
        notes: "TV web client; reliable fallback for uploaded and explicit songs.",
    },
    PlaybackClientManifest {
        id: "WEB_CREATOR",
        client_key: "WEB_CREATOR",
        display_name: "Web Creator",
        lifecycle: ClientLifecycle::Stable,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 70,
        authentication: AuthenticationPolicy::Required,
        transports: &[PlaybackTransport::Direct],
        content: ClientContentCapabilities {
            normal: CapabilitySupport::Supported,
            explicit: CapabilitySupport::Supported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Supported,
            live: CapabilitySupport::Supported,
            uploads: CapabilitySupport::Supported,
        },
        notes: "Creator Studio client; handles age-restricted and personal upload streams.",
    },
    PlaybackClientManifest {
        id: "MWEB",
        client_key: "MWEB",
        display_name: "Mobile Web",
        lifecycle: ClientLifecycle::Stable,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 65,
        authentication: AuthenticationPolicy::Optional,
        transports: &[PlaybackTransport::Direct],
        content: ClientContentCapabilities {
            normal: CapabilitySupport::Supported,
            explicit: CapabilitySupport::Supported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Supported,
            live: CapabilitySupport::Supported,
            uploads: CapabilitySupport::Supported,
        },
        notes: "Mobile web client fallback.",
    },
    PlaybackClientManifest {
        id: "WEB_EMBEDDED_PLAYER",
        client_key: "WEB_EMBEDDED_PLAYER",
        display_name: "Web Embedded Player",
        lifecycle: ClientLifecycle::Stable,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 60,
        authentication: AuthenticationPolicy::Optional,
        transports: &[PlaybackTransport::Direct],
        content: ClientContentCapabilities {
            normal: CapabilitySupport::Supported,
            explicit: CapabilitySupport::Supported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Supported,
            live: CapabilitySupport::Supported,
            uploads: CapabilitySupport::Unsupported,
        },
        notes: "Third-party embedded iframe client; unblocks embeddable content.",
    },
    PlaybackClientManifest {
        id: "WEB_KIDS",
        client_key: "WEB_KIDS",
        display_name: "Web Kids",
        lifecycle: ClientLifecycle::Experimental,
        selection_mode: ClientSelectionMode::Automatic,
        priority: 55,
        authentication: AuthenticationPolicy::Unsupported,
        transports: &[PlaybackTransport::Direct],
        content: ClientContentCapabilities {
            normal: CapabilitySupport::Unsupported,
            explicit: CapabilitySupport::Unsupported,
            kids: CapabilitySupport::Supported,
            age_restricted: CapabilitySupport::Unsupported,
            live: CapabilitySupport::Unsupported,
            uploads: CapabilitySupport::Unsupported,
        },
        notes: "Specialized client for made-for-kids content.",
    },
];

#[derive(Debug, Clone, Default)]
pub struct ContentAwareFallbackStrategy;

impl ContentAwareFallbackStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Select and prioritize clients for stream extraction given the content hints and auth state.
    pub fn select_clients<'a>(
        &self,
        clients: &'a Clients,
        hints: &ContentHints,
        authenticated: bool,
        excluded_clients: &HashSet<String>,
    ) -> Vec<SelectedClient<'a>> {
        let req = ClientSelectionRequest {
            hints: hints.clone(),
            authenticated,
            excluded_clients: excluded_clients.clone(),
            ..Default::default()
        };
        self.select_clients_detailed(clients, &req, None).candidates
    }

    /// Select candidates and collect rejected clients with rich diagnostics matching the Kotlin ContentAwareFallbackStrategy.
    pub fn select_clients_detailed<'a>(
        &self,
        clients: &'a Clients,
        request: &ClientSelectionRequest,
        health_monitor: Option<&dyn ClientHealthMonitor>,
    ) -> ClientSelectionResult<'a> {
        let mut rejected = Vec::new();

        // 1. Manual override check
        if let Some(ref override_id) = request.hints.playback_client_override_id {
            if let Some(m) = MANIFESTS.iter().find(|m| m.id == override_id) {
                if !request.excluded_clients.contains(m.id) && !request.excluded_clients.contains(m.client_key) {
                    if let Some(client) = clients.get(m.client_key) {
                        return ClientSelectionResult {
                            candidates: vec![SelectedClient {
                                client,
                                manifest: m,
                                score: i32::MAX,
                                reasons: vec!["manual override".into()],
                            }],
                            rejected,
                        };
                    }
                } else {
                    rejected.push(RejectedClient {
                        manifest: m,
                        reasons: vec!["manual override excluded or failed".into()],
                    });
                }
            }
        }

        let mut candidates = Vec::new();
        let scope = ClientHealthScope::from_hints(&request.hints, request.authenticated);

        for m in MANIFESTS {
            if m.selection_mode != ClientSelectionMode::Automatic {
                continue;
            }
            if request.excluded_clients.contains(m.id) || request.excluded_clients.contains(m.client_key) {
                rejected.push(RejectedClient {
                    manifest: m,
                    reasons: vec!["excluded by caller".into()],
                });
                continue;
            }

            let mut rejection_reasons = Vec::new();
            if m.lifecycle == ClientLifecycle::Broken {
                rejection_reasons.push("client marked broken".into());
            }
            if m.authentication == AuthenticationPolicy::Required && !request.authenticated {
                rejection_reasons.push("login required".into());
            }
            if request.hints.is_uploaded == Some(true) && !request.authenticated {
                rejection_reasons.push("uploads require login".into());
            }
            if request.hints.is_uploaded == Some(true)
                && m.content.uploads == CapabilitySupport::Unsupported
            {
                rejection_reasons.push("content type unsupported (uploads)".into());
            }
            if request.hints.is_kids_content == Some(true)
                && m.content.kids == CapabilitySupport::Unsupported
            {
                rejection_reasons.push("content type unsupported (kids)".into());
            }
            if request.hints.is_explicit == Some(true)
                && m.content.explicit == CapabilitySupport::Unsupported
            {
                rejection_reasons.push("content type unsupported (explicit)".into());
            }
            if request.hints.is_age_restricted == Some(true)
                && m.content.age_restricted == CapabilitySupport::Unsupported
            {
                rejection_reasons.push("content type unsupported (age restricted)".into());
            }
            if request.hints.want_video
                && !m.transports.contains(&PlaybackTransport::Direct)
                && !m.transports.contains(&PlaybackTransport::Sabr)
            {
                rejection_reasons.push("transport cannot satisfy video request".into());
            }

            let Some(client) = clients.get(m.client_key) else {
                rejection_reasons.push("client not found in catalog".into());
                rejected.push(RejectedClient {
                    manifest: m,
                    reasons: rejection_reasons,
                });
                continue;
            };

            if !rejection_reasons.is_empty() {
                rejected.push(RejectedClient {
                    manifest: m,
                    reasons: rejection_reasons,
                });
                continue;
            }

            // Scoring
            let mut score = m.priority;
            let mut reasons = vec![format!("base={}", m.priority)];

            match request.transport_preference {
                PlaybackTransportPreference::Direct => {
                    if m.transports.contains(&PlaybackTransport::Direct) {
                        score += 30;
                        reasons.push("direct-preference=+30".into());
                    } else {
                        score -= 30;
                        reasons.push("direct-preference=-30".into());
                    }
                }
                PlaybackTransportPreference::Sabr => {
                    if m.transports.contains(&PlaybackTransport::Sabr) {
                        score += 30;
                        reasons.push("sabr-preference=+30".into());
                    } else {
                        score -= 30;
                        reasons.push("sabr-preference=-30".into());
                    }
                }
                PlaybackTransportPreference::Hls => {
                    if m.transports.contains(&PlaybackTransport::Hls) {
                        score += 30;
                        reasons.push("hls-preference=+30".into());
                    } else {
                        score -= 30;
                        reasons.push("hls-preference=-30".into());
                    }
                }
                PlaybackTransportPreference::Auto => {
                    if m.transports.contains(&PlaybackTransport::Direct) {
                        score += 10;
                        reasons.push("direct-fast-path=+10".into());
                    }
                }
            }

            if request.hints.is_uploaded == Some(true) {
                if m.id == "TVHTML5" {
                    score += 50;
                    reasons.push("preferred upload client=+50".into());
                } else if m.id == "WEB_CREATOR" {
                    score += 40;
                    reasons.push("creator upload support=+40".into());
                }
            }

            if request.hints.is_explicit == Some(true) {
                if m.content.explicit == CapabilitySupport::Supported {
                    score += 20;
                    reasons.push("explicit supported=+20".into());
                } else if m.content.explicit == CapabilitySupport::Limited {
                    score -= 10;
                    reasons.push("explicit limited=-10".into());
                }
            }

            if request.hints.is_kids_content == Some(true) && m.id == "WEB_KIDS" {
                score += 50;
                reasons.push("specialized kids client=+50".into());
            }

            if request.authenticated && client.login_supported {
                score += 15;
                reasons.push("authenticated session bonus=+15".into());
            }

            if request.hints.want_video {
                if m.client_key == "WEB_REMIX" || m.client_key == "WEB_CREATOR" {
                    score -= 40;
                    reasons.push("web-remix-or-creator-video-demoted=-40".into());
                } else if m.client_key.starts_with("TVHTML5")
                    || m.client_key.starts_with("VISIONOS")
                    || m.client_key.starts_with("ANDROID_VR")
                {
                    score += 35;
                    reasons.push("video-dedicated-client-preferred=+35".into());
                }
            }

            match m.lifecycle {
                ClientLifecycle::Stable => {}
                ClientLifecycle::Canary => {
                    score -= 15;
                    reasons.push("canary=-15".into());
                }
                ClientLifecycle::Experimental => {
                    score -= 10;
                    reasons.push("experimental=-10".into());
                }
                ClientLifecycle::Unreleased => {
                    score -= 5;
                    reasons.push("unreleased=-5".into());
                }
                ClientLifecycle::Deprecated => {
                    score -= 20;
                    reasons.push("deprecated=-20".into());
                }
                ClientLifecycle::Broken => {
                    score -= 100;
                    reasons.push("broken=-100".into());
                }
            }

            if let Some(monitor) = health_monitor {
                let adj = monitor.score_adjustment(m.id, Some(&scope));
                if adj != 0 {
                    score += adj;
                    reasons.push(format!("runtime-health={:+}", adj));
                }
            }

            candidates.push(SelectedClient {
                client,
                manifest: m,
                score,
                reasons,
            });
        }

        candidates.sort_by(|a, b| {
            let a_has_direct = a.manifest.transports.contains(&PlaybackTransport::Direct);
            let b_has_direct = b.manifest.transports.contains(&PlaybackTransport::Direct);

            b_has_direct
                .cmp(&a_has_direct)
                .then(b.score.cmp(&a.score))
                .then(a.manifest.id.cmp(b.manifest.id))
        });

        rejected.sort_by(|a, b| a.manifest.id.cmp(b.manifest.id));

        ClientSelectionResult {
            candidates,
            rejected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_clients_prefers_direct_for_normal_content() {
        let clients = Clients::bundled();
        let strategy = ContentAwareFallbackStrategy::new();
        let hints = ContentHints::default();
        let excluded = HashSet::new();

        let selected = strategy.select_clients(&clients, &hints, false, &excluded);
        assert!(!selected.is_empty());
        // For normal anonymous content, VISIONOS variants should be ranked high
        let first = &selected[0];
        assert!(first.manifest.transports.contains(&PlaybackTransport::Direct));
    }

    #[test]
    fn select_clients_filters_unsupported_for_uploads() {
        let clients = Clients::bundled();
        let strategy = ContentAwareFallbackStrategy::new();
        let mut hints = ContentHints::default();
        hints.is_uploaded = Some(true);
        let excluded = HashSet::new();

        let selected = strategy.select_clients(&clients, &hints, true, &excluded);
        // Anonymous VR and visionOS don't support uploads and should be filtered out
        for s in &selected {
            assert_ne!(s.manifest.id, "VISIONOS_0_1");
            assert_ne!(s.manifest.id, "ANDROID_VR_1_65_10");
            assert_eq!(s.manifest.content.uploads, CapabilitySupport::Supported);
        }
    }

    #[test]
    fn select_clients_filters_unsupported_for_explicit() {
        let clients = Clients::bundled();
        let strategy = ContentAwareFallbackStrategy::new();
        let mut hints = ContentHints::default();
        hints.is_explicit = Some(true);
        let excluded = HashSet::new();

        let selected = strategy.select_clients(&clients, &hints, true, &excluded);
        for s in &selected {
            assert_ne!(
                s.manifest.content.explicit,
                CapabilitySupport::Unsupported,
                "client {} should not be selected for explicit track",
                s.manifest.id
            );
        }
    }

    #[test]
    fn select_clients_detailed_tracks_rejections_and_health() {
        struct TestMonitor;
        impl ClientHealthMonitor for TestMonitor {
            fn score_adjustment(&self, client_id: &str, _scope: Option<&ClientHealthScope>) -> i32 {
                if client_id == "VISIONOS_0_1" {
                    -50
                } else {
                    10
                }
            }
        }

        let clients = Clients::bundled();
        let strategy = ContentAwareFallbackStrategy::new();
        let req = ClientSelectionRequest {
            authenticated: false,
            transport_preference: PlaybackTransportPreference::Direct,
            ..Default::default()
        };

        let result = strategy.select_clients_detailed(&clients, &req, Some(&TestMonitor));
        assert!(!result.candidates.is_empty());
        assert!(!result.rejected.is_empty());

        // WEB_CREATOR requires login, so it must be in rejected
        let creator_rejected = result.rejected.iter().find(|r| r.manifest.id == "WEB_CREATOR");
        assert!(creator_rejected.is_some());
        assert!(creator_rejected.unwrap().reasons.iter().any(|r| r.contains("login required")));

        // Candidates must include health adjustments in reasons
        let visionos = result.candidates.iter().find(|c| c.manifest.id == "VISIONOS_0_1").unwrap();
        assert!(visionos.reasons.iter().any(|r| r == "runtime-health=-50"));
    }
}
