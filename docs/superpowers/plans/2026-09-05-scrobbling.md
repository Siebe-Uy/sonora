# Scrobbling (Last.fm + ListenBrainz) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Submit "now playing" and scrobble events to Last.fm and/or ListenBrainz as the user plays tracks, from any provider (Spotify, YouTube, local files), with each service connected independently.

**Architecture:** `music` crate gains a `Scrobbler` trait plus two implementations (`lastfm`, `listenbrainz`) that talk to each service's HTTP API — the same shape as the existing `LyricsProvider` submodules. `state` crate gains a `Scrobbling` entity, wired like `History`: it subscribes to `Playback`'s `StartedPlayback`/`EndedPlayback` events and observes position ticks to fire the two calls at the right moments. Settings persist the Last.fm session key/username and the ListenBrainz token; the Settings screen gets a new section to connect/disconnect each.

**Tech Stack:** Rust, `reqwest` (already a workspace dependency) for HTTP, a new `md5` crate for Last.fm's request signing, `gpui` entities for state, Fluent for UI copy.

**Spec:** `docs/superpowers/specs/2026-09-05-scrobbling-design.md`

## Global Constraints

- **No new automated tests.** This project's CLAUDE.md is explicit: "Do not add tests unless asked." Every task below verifies with `cargo check`/`cargo clippy` (and a manual smoke check for the final task) instead of a red/green TDD cycle — this is a deliberate deviation from this skill's default template, following the repo's own stated convention.
- Comments: essentially none. Only add one where a hidden constraint or non-obvious workaround needs it.
- `anyhow::Result` at boundaries, `.context("cannot …")` lowercase for errors that bubble up; `log::warn!` prefixed `"scrobble: "` for background failures, matching `History`'s `"history: "` prefix convention.
- Never hardcode a color/radius/size in UI code — read from `cx.theme()`.
- Never call `t!()` inside a constructor — store the Fluent key, resolve at render.
- Dependencies go in the root `Cargo.toml` `[workspace.dependencies]`, then `dep.workspace = true` in the crate.
- Conventional Commits for every commit: `type(scope): description`, imperative, lowercase, no trailing period, no body, no `Co-Authored-By`. Scopes used below: `music`, `state`, `views`, `i18n`.

---

### Task 1: `Scrobbler` trait + `md5` dependency

**Files:**
- Modify: `Cargo.toml:50` (workspace dependencies — insert alphabetically near `lofty`/`log`)
- Modify: `crates/music/Cargo.toml` (add `md5.workspace = true`)
- Modify: `crates/music/src/lib.rs` (add the trait near `LyricsProvider`, around line 150)

**Interfaces:**
- Produces: `music::Scrobbler` trait — `async fn now_playing(&self, track: &Track) -> Result<()>`, `async fn scrobble(&self, track: &Track, started_at: SystemTime) -> Result<()>`. Tasks 2 and 3 implement it.

- [ ] **Step 1: Add the `md5` workspace dependency**

In `Cargo.toml`, find this block (around line 50):

```toml
lofty = "0.25"
log = "0.4"
moka = { version = "0.12.15", features = ["future", "sync"] }
```

Insert `md5` between `log` and `moka`:

```toml
lofty = "0.25"
log = "0.4"
md5 = "0.7"
moka = { version = "0.12.15", features = ["future", "sync"] }
```

- [ ] **Step 2: Add `md5` to the `music` crate**

In `crates/music/Cargo.toml`, in the `[dependencies]` block, insert alphabetically:

```toml
lofty.workspace = true
kakasi.workspace = true
log.workspace = true
```

becomes:

```toml
lofty.workspace = true
kakasi.workspace = true
log.workspace = true
md5.workspace = true
```

- [ ] **Step 3: Add the `Scrobbler` trait**

In `crates/music/src/lib.rs`, find the `PlaybackFactory` trait (directly after `PlaybackEvents`, around line 203-205):

```rust
pub trait PlaybackFactory: Send + Sync {
    fn start(&self, config: PlaybackConfig) -> (Box<dyn Player>, Box<dyn PlaybackEvents>);
}
```

Add directly after it:

```rust
#[async_trait]
pub trait Scrobbler: Send + Sync {
    async fn now_playing(&self, track: &Track) -> Result<()>;
    async fn scrobble(&self, track: &Track, started_at: std::time::SystemTime) -> Result<()>;
}
```

`Track` and `async_trait` are already imported at the top of this file (`use async_trait::async_trait;` at line 22, `Track` defined in `models.rs` and re-exported at crate root — check the existing `use` block if `Track` isn't already in scope at this point in the file; if not, no import is needed since `Track` is defined in the same crate at `crate::models::Track` and re-exported via `pub use models::*;`).

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p music`
Expected: no errors (the trait has no implementers yet, which is fine — traits don't need implementers to compile).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/music/Cargo.toml crates/music/src/lib.rs
git commit -m "feat(music): add the Scrobbler trait"
```

---

### Task 2: Last.fm client

**Files:**
- Create: `crates/music/src/lastfm.rs`
- Modify: `crates/music/src/lib.rs` (register the module)

**Interfaces:**
- Consumes: `music::Scrobbler` (Task 1), `music::Track`, `music::ArtistRef`.
- Produces: `music::lastfm::API_KEY: &str`, `music::lastfm::LastfmClient` (implements `Scrobbler`, constructed with `LastfmClient::new(session_key: String)`), `music::lastfm::request_token() -> Result<String>`, `music::lastfm::auth_url(token: &str) -> String`, `music::lastfm::exchange_session(token: &str) -> Result<(String, String)>` (returns `(session_key, username)`). Task 6 (`state::Scrobbling`) calls all of these.

- [ ] **Step 1: Write the client**

Create `crates/music/src/lastfm.rs`:

```rust
use std::collections::BTreeMap;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use serde::Deserialize;

use crate::{Scrobbler, Track};

pub const API_KEY: &str = "febf8084eb500b93caa54e436264614d";
const API_SECRET: &str = "79d3a3d1ea0db85e46760db872587100";
const ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";

pub struct LastfmClient {
    http: reqwest::Client,
    session_key: String,
}

impl LastfmClient {
    pub fn new(session_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            session_key,
        }
    }
}

fn artist_of(track: &Track) -> String {
    track
        .artist_refs
        .first()
        .map(|artist| artist.name.clone())
        .unwrap_or_else(|| track.artists.clone())
}

fn sign(params: &BTreeMap<&'static str, String>) -> String {
    let mut signed = String::new();
    for (key, value) in params {
        signed.push_str(key);
        signed.push_str(value);
    }
    signed.push_str(API_SECRET);
    format!("{:x}", md5::compute(signed))
}

async fn signed_get<T: for<'de> Deserialize<'de>>(
    params: BTreeMap<&'static str, String>,
) -> Result<T> {
    let api_sig = sign(&params);
    let mut query: Vec<(&str, String)> = params.into_iter().collect();
    query.push(("api_sig", api_sig));
    query.push(("format", "json".to_owned()));

    reqwest::Client::new()
        .get(ENDPOINT)
        .query(&query)
        .send()
        .await
        .context("cannot reach last.fm")?
        .json()
        .await
        .context("cannot read the last.fm response")
}

async fn signed_post(http: &reqwest::Client, params: BTreeMap<&'static str, String>) -> Result<()> {
    let api_sig = sign(&params);
    let mut form: Vec<(&str, String)> = params.into_iter().collect();
    form.push(("api_sig", api_sig));
    form.push(("format", "json".to_owned()));

    let response = http
        .post(ENDPOINT)
        .form(&form)
        .send()
        .await
        .context("cannot reach last.fm")?;
    if !response.status().is_success() {
        anyhow::bail!("last.fm answered with status {}", response.status());
    }
    Ok(())
}

pub async fn request_token() -> Result<String> {
    #[derive(Deserialize)]
    struct Response {
        token: String,
    }
    let mut params = BTreeMap::new();
    params.insert("method", "auth.getToken".to_owned());
    params.insert("api_key", API_KEY.to_owned());
    let response: Response = signed_get(params).await?;
    Ok(response.token)
}

pub fn auth_url(token: &str) -> String {
    format!("https://www.last.fm/api/auth/?api_key={API_KEY}&token={token}")
}

pub async fn exchange_session(token: &str) -> Result<(String, String)> {
    #[derive(Deserialize)]
    struct Session {
        name: String,
        key: String,
    }
    #[derive(Deserialize)]
    struct Response {
        session: Session,
    }
    let mut params = BTreeMap::new();
    params.insert("method", "auth.getSession".to_owned());
    params.insert("api_key", API_KEY.to_owned());
    params.insert("token", token.to_owned());
    let response: Response = signed_get(params).await?;
    Ok((response.session.key, response.session.name))
}

#[async_trait]
impl Scrobbler for LastfmClient {
    async fn now_playing(&self, track: &Track) -> Result<()> {
        let mut params = BTreeMap::new();
        params.insert("method", "track.updateNowPlaying".to_owned());
        params.insert("api_key", API_KEY.to_owned());
        params.insert("sk", self.session_key.clone());
        params.insert("artist", artist_of(track));
        params.insert("track", track.name.clone());
        params.insert("album", track.album.clone());
        signed_post(&self.http, params).await
    }

    async fn scrobble(&self, track: &Track, started_at: SystemTime) -> Result<()> {
        let timestamp = started_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let mut params = BTreeMap::new();
        params.insert("method", "track.scrobble".to_owned());
        params.insert("api_key", API_KEY.to_owned());
        params.insert("sk", self.session_key.clone());
        params.insert("artist", artist_of(track));
        params.insert("track", track.name.clone());
        params.insert("album", track.album.clone());
        params.insert("timestamp", timestamp);
        signed_post(&self.http, params).await
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/music/src/lib.rs`, find the top-level `mod`/`pub use` area. Add (alongside wherever other provider submodules like `spotify`/`youtube`/`local` are declared — if none are declared with `mod` in this file because they're wired from `main.rs`, add it near the top of the file after the existing `use` statements):

```rust
pub mod lastfm;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p music`
Expected: no errors. If `Track`/`ArtistRef` field names don't match (e.g. `artist_refs` or `artists`), the compiler error will name the exact mismatch — `crates/music/src/models.rs:54-74` has the authoritative `Track` struct.

Run: `cargo clippy -p music --all-targets -- -D warnings`
Expected: clean. (`BTreeMap` iteration order is what makes `sign()` deterministic — don't switch it to `HashMap`.)

- [ ] **Step 4: Commit**

```bash
git add crates/music/src/lastfm.rs crates/music/src/lib.rs
git commit -m "feat(music): add a last.fm scrobbling client"
```

---

### Task 3: ListenBrainz client

**Files:**
- Create: `crates/music/src/listenbrainz.rs`
- Modify: `crates/music/src/lib.rs` (register the module)

**Interfaces:**
- Consumes: `music::Scrobbler` (Task 1), `music::Track`, `music::ArtistRef`.
- Produces: `music::listenbrainz::ListenBrainzClient` (implements `Scrobbler`, constructed with `ListenBrainzClient::new(token: String)`), `music::listenbrainz::validate_token(token: &str) -> Result<bool>`. Task 6 calls both.

- [ ] **Step 1: Write the client**

Create `crates/music/src/listenbrainz.rs`:

```rust
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Scrobbler, Track};

const SUBMIT_ENDPOINT: &str = "https://api.listenbrainz.org/1/submit-listens";
const VALIDATE_ENDPOINT: &str = "https://api.listenbrainz.org/1/validate-token";

pub struct ListenBrainzClient {
    http: reqwest::Client,
    token: String,
}

impl ListenBrainzClient {
    pub fn new(token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token,
        }
    }

    async fn submit(&self, body: Value) -> Result<()> {
        let response = self
            .http
            .post(SUBMIT_ENDPOINT)
            .header("Authorization", format!("Token {}", self.token))
            .json(&body)
            .send()
            .await
            .context("cannot reach listenbrainz")?;
        if !response.status().is_success() {
            anyhow::bail!("listenbrainz answered with status {}", response.status());
        }
        Ok(())
    }
}

fn artist_of(track: &Track) -> String {
    track
        .artist_refs
        .first()
        .map(|artist| artist.name.clone())
        .unwrap_or_else(|| track.artists.clone())
}

fn listen_payload(track: &Track, listen_type: &str, listened_at: Option<u64>) -> Value {
    let mut entry = json!({
        "track_metadata": {
            "artist_name": artist_of(track),
            "track_name": track.name,
            "release_name": track.album,
            "additional_info": {
                "duration_ms": track.duration.as_millis() as u64,
            },
        },
    });
    if let Some(listened_at) = listened_at {
        entry["listened_at"] = json!(listened_at);
    }
    json!({
        "listen_type": listen_type,
        "payload": [entry],
    })
}

pub async fn validate_token(token: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct Response {
        valid: bool,
    }
    let response: Response = reqwest::Client::new()
        .get(VALIDATE_ENDPOINT)
        .header("Authorization", format!("Token {token}"))
        .send()
        .await
        .context("cannot reach listenbrainz")?
        .json()
        .await
        .context("cannot read the listenbrainz response")?;
    Ok(response.valid)
}

#[async_trait]
impl Scrobbler for ListenBrainzClient {
    async fn now_playing(&self, track: &Track) -> Result<()> {
        self.submit(listen_payload(track, "playing_now", None)).await
    }

    async fn scrobble(&self, track: &Track, started_at: SystemTime) -> Result<()> {
        let listened_at = started_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.submit(listen_payload(track, "single", Some(listened_at)))
            .await
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/music/src/lib.rs`, next to the `pub mod lastfm;` added in Task 2:

```rust
pub mod lastfm;
pub mod listenbrainz;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p music`
Expected: no errors.

Run: `cargo clippy -p music --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/music/src/listenbrainz.rs crates/music/src/lib.rs
git commit -m "feat(music): add a listenbrainz scrobbling client"
```

---

### Task 4: Settings fields

**Files:**
- Modify: `crates/state/src/settings.rs`

**Interfaces:**
- Produces: `AppSettings::lastfm_session_key(&self) -> &str`, `AppSettings::set_lastfm_session_key(&mut self, impl Into<String>, &mut Context<Self>)`, `AppSettings::lastfm_username(&self) -> &str`, `AppSettings::set_lastfm_username(&mut self, impl Into<String>, &mut Context<Self>)`, `AppSettings::listenbrainz_token(&self) -> &str`, `AppSettings::set_listenbrainz_token(&mut self, impl Into<String>, &mut Context<Self>)`. Task 5/6 (`state::Scrobbling`) and Task 9 (settings UI) read and write these.

- [ ] **Step 1: Add the fields to `Values`**

In `crates/state/src/settings.rs`, find the `Values` struct (around line 147-192). It ends with:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<Frame>,
    appearance: Appearance,
}
```

Add three fields before `appearance`:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<Frame>,
    #[serde(default)]
    lastfm_session_key: String,
    #[serde(default)]
    lastfm_username: String,
    #[serde(default)]
    listenbrainz_token: String,
    appearance: Appearance,
}
```

(The struct already carries `#[serde(default)]` at the container level — see line 148 `#[serde(default)] struct Values` — so the per-field `#[serde(default)]` here is redundant but harmless; keep it for clarity since these are the newest fields.)

- [ ] **Step 2: Add the defaults**

In the same file, find `impl Default for Values` (around line 214-254). It ends with:

```rust
            resume: None,
            window: None,
            appearance: Appearance::default(),
        }
    }
}
```

Add the three new fields:

```rust
            resume: None,
            window: None,
            lastfm_session_key: String::new(),
            lastfm_username: String::new(),
            listenbrainz_token: String::new(),
            appearance: Appearance::default(),
        }
    }
}
```

- [ ] **Step 3: Add the accessors**

Find `pub fn theme_overrides` (around line 510-513):

```rust
    pub fn theme_overrides(&self) -> &ThemeOverrides {
        &self.values.appearance.theme_overrides
    }
```

Add directly after it:

```rust
    pub fn lastfm_session_key(&self) -> &str {
        &self.values.lastfm_session_key
    }

    pub fn lastfm_username(&self) -> &str {
        &self.values.lastfm_username
    }

    pub fn listenbrainz_token(&self) -> &str {
        &self.values.listenbrainz_token
    }
```

- [ ] **Step 4: Add the setters**

Find `pub fn set_provider` (around line 768-775):

```rust
    pub fn set_provider(&mut self, provider: impl Into<String>, cx: &mut Context<Self>) {
        let provider = provider.into();
        if self.values.provider == provider {
            return;
        }
        self.values.provider = provider;
        self.schedule_save(cx);
    }
```

Add directly after it:

```rust
    pub fn set_lastfm_session_key(&mut self, key: impl Into<String>, cx: &mut Context<Self>) {
        let key = key.into();
        if self.values.lastfm_session_key == key {
            return;
        }
        self.values.lastfm_session_key = key;
        self.schedule_save(cx);
    }

    pub fn set_lastfm_username(&mut self, username: impl Into<String>, cx: &mut Context<Self>) {
        let username = username.into();
        if self.values.lastfm_username == username {
            return;
        }
        self.values.lastfm_username = username;
        self.schedule_save(cx);
    }

    pub fn set_listenbrainz_token(&mut self, token: impl Into<String>, cx: &mut Context<Self>) {
        let token = token.into();
        if self.values.listenbrainz_token == token {
            return;
        }
        self.values.listenbrainz_token = token;
        self.schedule_save(cx);
    }
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p state`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/state/src/settings.rs
git commit -m "feat(state): persist last.fm and listenbrainz credentials"
```

---

### Task 5: `Scrobbling` entity — playback wiring

**Files:**
- Create: `crates/state/src/scrobble.rs`
- Modify: `crates/state/src/lib.rs` (register the module only — global wiring is Task 7)

**Interfaces:**
- Consumes: `music::Scrobbler`, `music::lastfm::LastfmClient`, `music::listenbrainz::ListenBrainzClient` (Tasks 1-3); `AppSettings::lastfm_session_key/listenbrainz_token` (Task 4); `crate::playback::PlaybackEvent`, `Playback::track()`, `Playback::position()` (existing); `crate::Io`, `crate::join` (existing).
- Produces: `state::Scrobbling` entity with `Scrobbling::new(settings: Entity<AppSettings>, playback: Entity<Playback>, io: Io, cx: &mut Context<Self>) -> Self`. Task 6 adds the connect/disconnect API on the same struct; Task 7 wires it into `Sonora`.

- [ ] **Step 1: Write the entity's playback-driven core**

Create `crates/state/src/scrobble.rs`:

```rust
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
    pending_lastfm_token: Option<String>,
    lastfm_task: Option<Task<()>>,
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
```

- [ ] **Step 2: Register the module**

In `crates/state/src/lib.rs`, find the `mod` list (lines 1-23) and insert `scrobble` alphabetically between `remote` and `search`:

```rust
mod remote;
mod scrobble;
mod search;
```

Then find the matching `pub use` block (lines 25-46) and insert `Scrobbling` in the same position:

```rust
pub use remote::{Remote, attach as attach_remote};
pub use scrobble::Scrobbling;
pub use search::{AlbumHit, ArtistHit, Hit, Kind, PlaylistHit, Search};
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p state`
Expected: no errors. `Scrobbling` isn't constructed anywhere yet (that's Task 7), which is fine — an unused-but-public struct doesn't warn.

Run: `cargo clippy -p state --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/state/src/scrobble.rs crates/state/src/lib.rs
git commit -m "feat(state): add the Scrobbling entity's playback wiring"
```

---

### Task 6: `Scrobbling` — connect/disconnect API

**Files:**
- Modify: `crates/state/src/scrobble.rs`

**Interfaces:**
- Consumes: `music::lastfm::{request_token, auth_url, exchange_session}`, `music::listenbrainz::validate_token` (Task 2/3); `AppSettings::set_lastfm_session_key/set_lastfm_username/set_listenbrainz_token` (Task 4); `crate::{Outcome, Toasts, join}` (existing).
- Produces: `Scrobbling::lastfm_username(&self, cx) -> Option<SharedString>`, `Scrobbling::lastfm_awaiting_confirmation(&self) -> bool`, `Scrobbling::listenbrainz_connected(&self, cx) -> bool`, `Scrobbling::connect_lastfm(&mut self, cx)`, `Scrobbling::confirm_lastfm(&mut self, cx)`, `Scrobbling::disconnect_lastfm(&mut self, cx)`, `Scrobbling::set_listenbrainz_token(&mut self, token: String, cx)`, `Scrobbling::disconnect_listenbrainz(&mut self, cx)`. Task 9 (settings UI) calls all of these.

- [ ] **Step 1: Add the toast/i18n keys this step needs, and the imports**

At the top of `crates/state/src/scrobble.rs`, replace:

```rust
use gpui::{Context, Entity, Task};
use music::lastfm::LastfmClient;
use music::listenbrainz::ListenBrainzClient;
use music::{Scrobbler, Track};

use crate::playback::PlaybackEvent;
use crate::{AppSettings, Io, Playback};
```

with:

```rust
use gpui::{App, Context, Entity, SharedString, Task};
use music::lastfm::LastfmClient;
use music::listenbrainz::ListenBrainzClient;
use music::{Scrobbler, Track};

use crate::playback::PlaybackEvent;
use crate::{AppSettings, Io, Outcome, Playback, Toasts, join};
```

`App` is needed because `lastfm_username`/`listenbrainz_connected` below take `&App`, not `&Context<Self>` — they're read from `SettingsView`'s own context (Task 9), which is a `Context<SettingsView>`, not a `Context<Scrobbling>`. `Context<T>` derefs to `App`, so passing either entity's context works once the parameter type is the common `&App`.

- [ ] **Step 2: Add the connect/disconnect methods**

At the end of the `impl Scrobbling` block in `crates/state/src/scrobble.rs` (after `check_threshold`, before the closing `}`), add:

```rust
    pub fn lastfm_username(&self, cx: &App) -> Option<SharedString> {
        let username = self.settings.read(cx).lastfm_username().to_owned();
        (!username.is_empty()).then(|| SharedString::from(username))
    }

    pub fn lastfm_awaiting_confirmation(&self) -> bool {
        self.pending_lastfm_token.is_some()
    }

    pub fn listenbrainz_connected(&self, cx: &App) -> bool {
        !self.settings.read(cx).listenbrainz_token().is_empty()
    }

    pub fn connect_lastfm(&mut self, cx: &mut Context<Self>) {
        if self.lastfm_task.is_some() {
            return;
        }
        let io = self.io.clone();
        self.lastfm_task = Some(cx.spawn(async move |this, cx| {
            let requested = join(io.spawn(async move { music::lastfm::request_token().await })).await;
            this.update(cx, |this, cx| {
                this.lastfm_task = None;
                match requested {
                    Ok(token) => {
                        let url = music::lastfm::auth_url(&token);
                        this.pending_lastfm_token = Some(token);
                        cx.open_url(&url);
                    }
                    Err(error) => {
                        log::warn!("scrobble: cannot request a last.fm token: {error:#}");
                        Toasts::show(Outcome::Failed, "toast-lastfm-failed", cx);
                    }
                }
            })
            .ok();
        }));
    }

    pub fn confirm_lastfm(&mut self, cx: &mut Context<Self>) {
        if self.lastfm_task.is_some() {
            return;
        }
        let Some(token) = self.pending_lastfm_token.clone() else {
            return;
        };
        let io = self.io.clone();
        self.lastfm_task = Some(cx.spawn(async move |this, cx| {
            let exchanged = join(io.spawn(async move { music::lastfm::exchange_session(&token).await })).await;
            this.update(cx, |this, cx| {
                this.lastfm_task = None;
                this.pending_lastfm_token = None;
                match exchanged {
                    Ok((session_key, username)) => {
                        this.settings.update(cx, |settings, cx| {
                            settings.set_lastfm_session_key(session_key, cx);
                            settings.set_lastfm_username(username.clone(), cx);
                        });
                        Toasts::about(Outcome::Done, "toast-lastfm-connected", username, cx);
                    }
                    Err(error) => {
                        log::warn!("scrobble: cannot complete last.fm sign-in: {error:#}");
                        Toasts::show(Outcome::Failed, "toast-lastfm-failed", cx);
                    }
                }
            })
            .ok();
        }));
    }

    pub fn disconnect_lastfm(&mut self, cx: &mut Context<Self>) {
        self.pending_lastfm_token = None;
        self.settings.update(cx, |settings, cx| {
            settings.set_lastfm_session_key(String::new(), cx);
            settings.set_lastfm_username(String::new(), cx);
        });
    }

    pub fn set_listenbrainz_token(&mut self, token: String, cx: &mut Context<Self>) {
        if self.listenbrainz_task.is_some() {
            return;
        }
        let checked = token.clone();
        let io = self.io.clone();
        self.listenbrainz_task = Some(cx.spawn(async move |this, cx| {
            let validated =
                join(io.spawn(async move { music::listenbrainz::validate_token(&checked).await }))
                    .await;
            this.update(cx, |this, cx| {
                this.listenbrainz_task = None;
                match validated {
                    Ok(true) => {
                        this.settings
                            .update(cx, |settings, cx| settings.set_listenbrainz_token(token, cx));
                        Toasts::show(Outcome::Done, "toast-listenbrainz-connected", cx);
                    }
                    Ok(false) => {
                        log::warn!("scrobble: listenbrainz rejected the token");
                        Toasts::show(Outcome::Failed, "toast-listenbrainz-failed", cx);
                    }
                    Err(error) => {
                        log::warn!("scrobble: cannot validate a listenbrainz token: {error:#}");
                        Toasts::show(Outcome::Failed, "toast-listenbrainz-failed", cx);
                    }
                }
            })
            .ok();
        }));
    }

    pub fn disconnect_listenbrainz(&mut self, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |settings, cx| settings.set_listenbrainz_token(String::new(), cx));
    }
```

Note: `join` is `pub(crate)` in `crates/state/src/lib.rs:84` and expects `JoinHandle<Result<T>>` — `io.spawn(async move { music::lastfm::request_token().await })` produces exactly that since `request_token()` returns `anyhow::Result<String>`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p state`
Expected: no errors.

Run: `cargo clippy -p state --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/state/src/scrobble.rs
git commit -m "feat(state): add scrobbling connect and disconnect flows"
```

---

### Task 7: Wire `Scrobbling` into the `Sonora` global

**Files:**
- Modify: `crates/state/src/lib.rs`

**Interfaces:**
- Consumes: `Scrobbling::new` (Task 5).
- Produces: `Sonora::global(cx).scrobbling: Entity<Scrobbling>`, reachable from every screen. Task 9 uses it.

- [ ] **Step 1: Add the field to `Sonora`**

In `crates/state/src/lib.rs`, find the `Sonora` struct (around line 88-99):

```rust
pub struct Sonora {
    pub session: Entity<Session>,
    pub cover: Entity<Cover>,
    pub library: Entity<Library>,
    pub history: Entity<History>,
    pub lyrics: Entity<Lyrics>,
    pub playback: Entity<Playback>,
    pub queue: Entity<Queue>,
    pub settings: Entity<AppSettings>,
    pub updates: Entity<Updates>,
    pub usage: Entity<Usage>,
}
```

Add `scrobbling` after `history`:

```rust
pub struct Sonora {
    pub session: Entity<Session>,
    pub cover: Entity<Cover>,
    pub library: Entity<Library>,
    pub history: Entity<History>,
    pub scrobbling: Entity<Scrobbling>,
    pub lyrics: Entity<Lyrics>,
    pub playback: Entity<Playback>,
    pub queue: Entity<Queue>,
    pub settings: Entity<AppSettings>,
    pub updates: Entity<Updates>,
    pub usage: Entity<Usage>,
}
```

- [ ] **Step 2: Construct it in `init`**

In the same file, find `init` (around line 109-150). It has:

```rust
    let history = cx.new(|cx| History::new(session.clone(), playback.clone(), io.clone(), cx));
    let lyrics = cx.new(|cx| {
```

Insert the construction between them:

```rust
    let history = cx.new(|cx| History::new(session.clone(), playback.clone(), io.clone(), cx));
    let scrobbling =
        cx.new(|cx| Scrobbling::new(settings.clone(), playback.clone(), io.clone(), cx));
    let lyrics = cx.new(|cx| {
```

Then find the `cx.set_global(Sonora { ... })` literal at the end of `init` and add `scrobbling,` after `history,`:

```rust
    cx.set_global(Sonora {
        session,
        cover,
        library,
        history,
        scrobbling,
        lyrics,
        playback,
        queue,
        settings,
        updates,
        usage,
```

(Leave the rest of that literal and the function's closing lines untouched.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p state`
Expected: no errors.

Run: `cargo check --workspace`
Expected: no errors — this also confirms `sonora`/`views` still build against the now-larger `Sonora` struct (they construct it only through `state::init`, so adding a field doesn't break call sites).

- [ ] **Step 4: Commit**

```bash
git add crates/state/src/lib.rs
git commit -m "feat(state): install the Scrobbling entity on the Sonora global"
```

---

### Task 8: i18n keys

**Files:**
- Modify: `assets/i18n/en-US/main.ftl`

**Interfaces:**
- Produces: the Fluent keys Task 9's UI resolves via `t!(...)` and the toasts Task 6 shows via `Toasts::show`/`Toasts::about`.

- [ ] **Step 1: Add the settings keys**

In `assets/i18n/en-US/main.ftl`, find:

```ftl
settings-provider-switch = Switch to
settings-sign-out = Sign out
```

Insert a new block directly after `settings-sign-out = Sign out` and before `settings-local-folder`:

```ftl
settings-provider-switch = Switch to
settings-sign-out = Sign out
settings-group-scrobbling = Scrobbling
settings-scrobbling = Send plays to Last.fm and ListenBrainz
settings-scrobbling-detail = Connect either service to log every track you finish
settings-lastfm-connect = Connect Last.fm
settings-lastfm-confirm = I've authorized
settings-lastfm-disconnect = Disconnect
settings-listenbrainz-token-hint = Paste your ListenBrainz user token
settings-listenbrainz-save = Save
settings-listenbrainz-disconnect = Disconnect
```

- [ ] **Step 2: Add the toast keys**

Find:

```ftl
toast-library-add-failed = { $name } could not be added to your library
toast-library-remove-failed = { $name } could not be removed from your library
```

Insert directly after:

```ftl
toast-library-add-failed = { $name } could not be added to your library
toast-library-remove-failed = { $name } could not be removed from your library
toast-lastfm-connected = Connected to Last.fm as { $name }
toast-lastfm-failed = Could not connect to Last.fm
toast-listenbrainz-connected = Connected to ListenBrainz
toast-listenbrainz-failed = Could not connect to ListenBrainz
```

- [ ] **Step 3: Verify the i18n test suite still passes**

Run: `cargo test -p i18n`
Expected: PASS — this crate's existing tests check that English carries every key referenced elsewhere and that no other locale invents a key English lacks; adding English-only keys here is exactly what the "a locale is allowed to lag" rule permits. Do not add `ru`/`uk`/`pl` translations unless you can do them properly — the fallback logs a warning and shows English instead, which is the intended behavior for a lagging locale.

- [ ] **Step 4: Commit**

```bash
git add assets/i18n/en-US/main.ftl
git commit -m "i18n: add scrobbling settings and toast copy"
```

---

### Task 9: Settings UI

**Files:**
- Modify: `crates/views/src/screens/settings.rs`

**Interfaces:**
- Consumes: `state::Scrobbling` and all of its public methods (Tasks 5-7); `ui::{Button, Input, Text}` (existing).

- [ ] **Step 1: Add the `Scrobbling` entity and a token `Input` to `SettingsView`**

In `crates/views/src/screens/settings.rs`, update the `state` import (line 18):

```rust
use state::{AppSettings, Failure, Playback, SYSTEM_FONT, Session, SessionState, Sonora};
```

becomes:

```rust
use state::{AppSettings, Failure, Playback, SYSTEM_FONT, Scrobbling, Session, SessionState, Sonora};
```

Then update the `SettingsView` struct (around line 123-137):

```rust
pub struct SettingsView {
    session: Entity<Session>,
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    tab: SettingsTab,
    scrollbar: Entity<Scrollbar>,
    opacity: ScrubberState,
    popovers: Popovers,
    browsers: Option<(&'static str, Vec<SharedString>)>,
    secret: Entity<Input>,
    languages: SearchPopup,
    typefaces: SearchPopup,
    typeface_faced: RefCell<HashSet<SharedString>>,
    installed: Option<Vec<SharedString>>,
}
```

becomes:

```rust
pub struct SettingsView {
    session: Entity<Session>,
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    scrobbling: Entity<Scrobbling>,
    tab: SettingsTab,
    scrollbar: Entity<Scrollbar>,
    opacity: ScrubberState,
    popovers: Popovers,
    browsers: Option<(&'static str, Vec<SharedString>)>,
    secret: Entity<Input>,
    listenbrainz_input: Entity<Input>,
    languages: SearchPopup,
    typefaces: SearchPopup,
    typeface_faced: RefCell<HashSet<SharedString>>,
    installed: Option<Vec<SharedString>>,
}
```

- [ ] **Step 2: Wire it in the constructor**

In the same file, find `SettingsView::new` (around line 140-177):

```rust
        let settings = Sonora::global(cx).settings.clone();
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
```

becomes:

```rust
        let settings = Sonora::global(cx).settings.clone();
        let scrobbling = Sonora::global(cx).scrobbling.clone();
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&scrobbling, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
```

And the struct literal at the end of `new`:

```rust
        Self {
            session,
            playback,
            settings,
            tab: SettingsTab::General,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            opacity: ScrubberState::new("opacity"),
            popovers: Popovers::default(),
            browsers: None,
            secret: cx.new(|cx| Input::new("login-cookie-hint", cx)),
            languages,
            typefaces,
            typeface_faced: RefCell::new(HashSet::new()),
            installed: None,
        }
```

becomes:

```rust
        Self {
            session,
            playback,
            settings,
            scrobbling,
            tab: SettingsTab::General,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            opacity: ScrubberState::new("opacity"),
            popovers: Popovers::default(),
            browsers: None,
            secret: cx.new(|cx| Input::new("login-cookie-hint", cx)),
            listenbrainz_input: cx.new(|cx| Input::new("settings-listenbrainz-token-hint", cx)),
            languages,
            typefaces,
            typeface_faced: RefCell::new(HashSet::new()),
            installed: None,
        }
```

- [ ] **Step 3: Add the section to the General tab**

Find `panel` (around line 185-197):

```rust
            SettingsTab::General => vec![
                Row::Item(self.startup_row(cx).into_any_element()),
                Row::Item(self.entries_row(cx).into_any_element()),
                Row::Item(self.language_row(cx).into_any_element()),
                self.title("settings-group-window", cx),
                Row::Item(self.tray_row(cx).into_any_element()),
                self.title("settings-group-accounts", cx),
                Row::Item(self.accounts_row(cx).into_any_element()),
                self.title("settings-group-library", cx),
                Row::Item(self.local_folder_row(cx).into_any_element()),
            ],
```

becomes:

```rust
            SettingsTab::General => vec![
                Row::Item(self.startup_row(cx).into_any_element()),
                Row::Item(self.entries_row(cx).into_any_element()),
                Row::Item(self.language_row(cx).into_any_element()),
                self.title("settings-group-window", cx),
                Row::Item(self.tray_row(cx).into_any_element()),
                self.title("settings-group-accounts", cx),
                Row::Item(self.accounts_row(cx).into_any_element()),
                self.title("settings-group-scrobbling", cx),
                Row::Item(self.scrobbling_row(cx).into_any_element()),
                self.title("settings-group-library", cx),
                Row::Item(self.local_folder_row(cx).into_any_element()),
            ],
```

- [ ] **Step 4: Write `scrobbling_row` and the two service cards**

In the same file, find `fn accounts_row` (around line 1308) and add these three methods directly before it:

```rust
    fn scrobbling_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(t!("settings-scrobbling"))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .text_size(theme.text(Text::Small))
                            .child(t!("settings-scrobbling-detail")),
                    ),
            )
            .child(self.lastfm_card(cx).into_any_element())
            .child(self.listenbrainz_card(cx).into_any_element())
    }

    fn lastfm_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let username = self.scrobbling.read(cx).lastfm_username(cx);
        let awaiting = self.scrobbling.read(cx).lastfm_awaiting_confirmation();

        div()
            .flex()
            .items_center()
            .gap_3()
            .p(theme.metrics.pad)
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Last.fm"))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .text_size(theme.text(Text::Small))
                            .child(username.clone().unwrap_or_else(|| t!("settings-provider-none"))),
                    ),
            )
            .child(match username.is_some() {
                true => Button::new("disconnect-lastfm")
                    .label(t!("settings-lastfm-disconnect"))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scrobbling
                            .update(cx, |scrobbling, cx| scrobbling.disconnect_lastfm(cx));
                    }))
                    .into_any_element(),
                false if awaiting => Button::new("confirm-lastfm")
                    .label(t!("settings-lastfm-confirm"))
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scrobbling
                            .update(cx, |scrobbling, cx| scrobbling.confirm_lastfm(cx));
                    }))
                    .into_any_element(),
                false => Button::new("connect-lastfm")
                    .label(t!("settings-lastfm-connect"))
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scrobbling
                            .update(cx, |scrobbling, cx| scrobbling.connect_lastfm(cx));
                    }))
                    .into_any_element(),
            })
    }

    fn listenbrainz_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let connected = self.scrobbling.read(cx).listenbrainz_connected(cx);

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p(theme.metrics.pad)
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().font_weight(FontWeight::MEDIUM).child("ListenBrainz"))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .text_size(theme.text(Text::Small))
                                    .child(match connected {
                                        true => t!("settings-provider-connected"),
                                        false => t!("settings-provider-none"),
                                    }),
                            ),
                    )
                    .when(connected, |this| {
                        this.child(
                            Button::new("disconnect-listenbrainz")
                                .label(t!("settings-listenbrainz-disconnect"))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.scrobbling.update(cx, |scrobbling, cx| {
                                        scrobbling.disconnect_listenbrainz(cx)
                                    });
                                })),
                        )
                    }),
            )
            .when(!connected, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.listenbrainz_input.clone())
                        .child(
                            Button::new("save-listenbrainz")
                                .label(t!("settings-listenbrainz-save"))
                                .small()
                                .outline()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let token = this.listenbrainz_input.read(cx).text().to_string();
                                    if token.trim().is_empty() {
                                        return;
                                    }
                                    this.listenbrainz_input
                                        .update(cx, |input, cx| input.set_text("", cx));
                                    this.scrobbling.update(cx, |scrobbling, cx| {
                                        scrobbling.set_listenbrainz_token(token, cx)
                                    });
                                })),
                        ),
                )
            })
    }

```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p views`
Expected: no errors. If `username.clone().unwrap_or_else(|| t!("settings-provider-none"))` doesn't type-check because `t!()` returns `SharedString` and `username` is `Option<SharedString>`, that's expected and already consistent — both arms are `SharedString`.

Run: `cargo clippy -p views --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/views/src/screens/settings.rs
git commit -m "feat(views): add a scrobbling section to settings"
```

---

### Task 10: Full workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt`
Expected: no diff, or only whitespace fixes in files touched by this plan.

- [ ] **Step 2: Full check**

Run: `cargo check --workspace`
Expected: no errors.

- [ ] **Step 3: Full clippy**

Run: `cargo clippy --workspace --all-targets`
Expected: clean, per this project's "clippy is always clean" rule.

- [ ] **Step 4: Manual smoke test**

Run: `cargo run --locked --package sonora`

In the running app:
1. Open Settings → General. Confirm a "Scrobbling" section appears between Accounts and Library, with a Last.fm card ("Connect Last.fm" button) and a ListenBrainz card (token field + Save).
2. Click "Connect Last.fm" — a browser tab should open to `last.fm/api/auth/?api_key=...&token=...`. Approve the app on that page, come back, click "I've authorized". The card should switch to showing your Last.fm username and a Disconnect button, and a toast should confirm the connection.
3. In a ListenBrainz account, copy your user token from listenbrainz.org/settings, paste it into the field, click Save. The card should switch to "Connected" + Disconnect, with a confirming toast. Paste an invalid string first to confirm the failure toast path.
4. Play a track longer than 30 seconds from any source (Spotify, YouTube, or an imported local file). Check `~/.local/state/sonora/sonora.log` (or the platform-equivalent `$XDG_STATE_HOME/sonora/sonora.log`) for `scrobble: ` lines — there should be none if both API calls succeed. Confirm on last.fm's profile page and/or listenbrainz's profile page that the track shows as "now playing" immediately and as a full scrobble after the threshold (half the track's length, capped at 4 minutes).
5. Disconnect both. Play another track. Confirm nothing errors and no scrobble appears on either service.

- [ ] **Step 5: Update the changelog**

In `CHANGELOG.md`, under `## [Unreleased]` → `### Added` (create that subsection if it doesn't exist yet), add:

```markdown
- Scrobble plays to Last.fm and ListenBrainz, connected independently in Settings.
```

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs: note scrobbling in the changelog"
```
