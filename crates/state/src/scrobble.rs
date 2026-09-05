use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{Context, Entity, Task};
use music::lastfm::LastfmClient;
use music::listenbrainz::ListenBrainzClient;
use music::{Scrobbler, Track};

use crate::playback::PlaybackEvent;
use crate::{AppSettings, Io, Playback};

const MIN_SCROBBLE_DURATION: Duration = Duration::from_secs(30);
const MAX_SCROBBLE_WAIT: Duration = Duration::from_secs(4 * 60);

pub struct Scrobbling {
    settings: Entity<AppSettings>,
    playback: Entity<Playback>,
    io: Io,
    lastfm: Option<Arc<LastfmClient>>,
    listenbrainz: Option<Arc<ListenBrainzClient>>,
    active: Option<String>,
    started_at: Option<SystemTime>,
    fired: bool,
    #[allow(dead_code)]
    pending_lastfm_token: Option<String>,
    #[allow(dead_code)]
    lastfm_task: Option<Task<()>>,
    #[allow(dead_code)]
    listenbrainz_task: Option<Task<()>>,
}

impl Scrobbling {
    pub fn new(
        settings: Entity<AppSettings>,
        playback: Entity<Playback>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&playback, |this, _, event, cx| match event {
            PlaybackEvent::StartedPlayback => this.start(cx),
            PlaybackEvent::EndedPlayback => this.stop(),
        })
        .detach();
        cx.observe(&playback, |this, _, cx| this.check_threshold(cx))
            .detach();
        cx.observe(&settings, |this, _, cx| this.rebuild(cx)).detach();

        let mut scrobbling = Self {
            settings,
            playback,
            io,
            lastfm: None,
            listenbrainz: None,
            active: None,
            started_at: None,
            fired: false,
            pending_lastfm_token: None,
            lastfm_task: None,
            listenbrainz_task: None,
        };
        scrobbling.rebuild(cx);
        scrobbling
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let (session_key, listenbrainz_token) = {
            let settings = self.settings.read(cx);
            (
                settings.lastfm_session_key().to_owned(),
                settings.listenbrainz_token().to_owned(),
            )
        };
        self.lastfm = match session_key.is_empty() {
            true => None,
            false => Some(Arc::new(LastfmClient::new(session_key))),
        };
        self.listenbrainz = match listenbrainz_token.is_empty() {
            true => None,
            false => Some(Arc::new(ListenBrainzClient::new(listenbrainz_token))),
        };
    }

    fn scrobblers(&self) -> Vec<Arc<dyn Scrobbler>> {
        let mut list: Vec<Arc<dyn Scrobbler>> = Vec::new();
        if let Some(client) = &self.lastfm {
            list.push(client.clone());
        }
        if let Some(client) = &self.listenbrainz {
            list.push(client.clone());
        }
        list
    }

    fn key_for(track: &Track) -> String {
        match &track.id {
            Some(id) => id.clone(),
            None => format!("{}\u{1}{}\u{1}{}", track.name, track.artists, track.album),
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.playback.read(cx).track().cloned() else {
            return;
        };
        let key = Self::key_for(&track);
        if self.active.as_ref() == Some(&key) {
            return;
        }
        self.active = Some(key);
        self.started_at = Some(SystemTime::now());
        self.fired = false;

        for scrobbler in self.scrobblers() {
            let track = track.clone();
            self.io.spawn(async move {
                if let Err(error) = scrobbler.now_playing(&track).await {
                    log::warn!("scrobble: cannot update now playing: {error:#}");
                }
            });
        }
    }

    fn stop(&mut self) {
        self.active = None;
        self.started_at = None;
        self.fired = false;
    }

    fn check_threshold(&mut self, cx: &mut Context<Self>) {
        if self.fired || self.active.is_none() {
            return;
        }
        let Some(track) = self.playback.read(cx).track().cloned() else {
            return;
        };
        if track.duration < MIN_SCROBBLE_DURATION {
            return;
        }
        let position = self.playback.read(cx).position();
        let threshold = (track.duration / 2).min(MAX_SCROBBLE_WAIT);
        if position < threshold {
            return;
        }
        let Some(started_at) = self.started_at else {
            return;
        };
        self.fired = true;

        for scrobbler in self.scrobblers() {
            let track = track.clone();
            self.io.spawn(async move {
                if let Err(error) = scrobbler.scrobble(&track, started_at).await {
                    log::warn!("scrobble: cannot submit a scrobble: {error:#}");
                }
            });
        }
    }
}
