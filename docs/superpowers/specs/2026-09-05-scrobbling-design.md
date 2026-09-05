# Scrobbling (Last.fm + ListenBrainz)

## Goal

Submit "now playing" and scrobble events to Last.fm and/or ListenBrainz as the
user plays tracks, from any provider (Spotify, YouTube, local files). Each
service connects independently; a play scrobbles to whichever are connected.

## Non-goals

- No scrobble-history import from either service.
- No offline retry queue for failed submissions — a failure is logged and
  dropped, matching `History`'s save-failure handling.
- No per-connection mute toggle distinct from connect/disconnect — being
  connected means active.

## Threshold rule

Shared by both services' own guidelines: scrobble a track once playback
reaches `min(duration / 2, 4 minutes)`, and only if `duration >= 30s`. The
"now playing" push has no such gate — it fires as soon as a track starts.

## `music` crate additions

No `gpui` dependency, following the existing `LyricsProvider` shape.

`crates/music/src/lib.rs` — new trait:

```rust
#[async_trait]
pub trait Scrobbler: Send + Sync {
    async fn now_playing(&self, track: &Track) -> Result<()>;
    async fn scrobble(&self, track: &Track, started_at: SystemTime) -> Result<()>;
}
```

### `crates/music/src/lastfm.rs`

```rust
pub const API_KEY: &str = "febf8084eb500b93caa54e436264614d";
const API_SECRET: &str = "79d3a3d1ea0db85e46760db872587100";

pub struct LastfmClient {
    session_key: String,
}
```

- `request_token() -> Result<String>` — `GET /2.0/?method=auth.getToken`.
- `auth_url(token: &str) -> String` — `https://www.last.fm/api/auth/?api_key=..&token=..`,
  opened with `cx.open_url` from the view layer.
- `exchange_session(token: &str) -> Result<(String, String)>` — `auth.getSession`,
  returns `(session_key, username)`.
- `LastfmClient::new(session_key: String)`, implements `Scrobbler`:
  `track.updateNowPlaying` / `track.scrobble`, both POST, signed with the
  standard Last.fm `api_sig` (params sorted, concatenated, `API_SECRET`
  appended, md5). `scrobble` sends `timestamp` = `started_at` as unix
  seconds.
- All requests carry `api_key=API_KEY`; session-authenticated calls also
  carry `sk=session_key`.

### `crates/music/src/listenbrainz.rs`

```rust
pub struct ListenBrainzClient {
    token: String,
}
```

- `validate_token(token: &str) -> Result<bool>` — `GET /1/validate-token`,
  `Authorization: Token <token>`.
- `ListenBrainzClient::new(token: String)`, implements `Scrobbler`:
  `POST /1/submit-listens` with `listen_type: "playing_now"` (no
  `listened_at`, no `duration_ms` required) for now-playing, and
  `listen_type: "single"` with `listened_at` = `started_at` unix seconds for
  scrobble. Track identification in `track_metadata`: `artist_name`,
  `track_name`, `release_name` (album), `additional_info.duration_ms`.

### Artist/track naming

Both clients take `&Track` and derive:
- artist: `track.artist_refs.first().map(|a| a.name.clone()).unwrap_or_else(|| track.artists.clone())`
- title: `track.name`
- album: `track.album`
- duration: `track.duration`

## `state` crate additions

`crates/state/src/scrobble.rs` — new `Scrobbling` entity, constructed and
wired the same way as `History` (`crates/state/src/history.rs`):

```rust
pub struct Scrobbling {
    settings: Entity<AppSettings>,
    playback: Entity<Playback>,
    io: Io,
    lastfm: Option<Arc<music::lastfm::LastfmClient>>,
    listenbrainz: Option<Arc<music::listenbrainz::ListenBrainzClient>>,
    pending_lastfm_token: Option<String>,
    active: Option<ScrobbleKey>,       // dedup, mirrors History::active
    started_at: Option<SystemTime>,
    fired: bool,                        // scrobble threshold already sent for `active`
    connecting: bool,
}
```

- Built from `settings.lastfm_session_key` / `settings.listenbrainz_token` on
  construction and whenever settings change (`cx.observe(&settings, ..)`),
  rebuilding the `Option<Arc<..>>` clients when a key/token is added or
  cleared.
- `cx.subscribe(&playback, ..)`:
  - `PlaybackEvent::StartedPlayback` → compute the new `ScrobbleKey` from
    `playback.track()`; if it differs from `active`, reset `fired`, record
    `started_at = SystemTime::now()`, and fire `now_playing` on every
    connected client via `io.spawn` (fire-and-forget, `log::warn!` on `Err`).
  - `PlaybackEvent::EndedPlayback` → clear `active`/`started_at`/`fired`.
- `cx.observe(&playback, ..)`: on every notify, if `active` is set and
  `!fired`, read `playback.position()` and the active track's `duration`;
  once `position >= min(duration / 2, 4 min)` and `duration >= 30s`, set
  `fired = true` and fire `scrobble(track, started_at)` on every connected
  client via `io.spawn`.
- `ScrobbleKey` = `track.id.clone()` when present, else a hash of
  `(name, artists, album)` for identifier-less local tracks — mirrors how
  `History` keys on `(provider, track_id)` but scrobbling has no provider
  concept, only track identity.
- Public API:
  - `fn lastfm_username(&self) -> Option<&str>`
  - `fn listenbrainz_connected(&self) -> bool`
  - `fn connect_lastfm(&mut self, cx)` → `io.spawn(request_token)`, on
    success stash `pending_lastfm_token`, return the auth URL for the view to
    open via `cx.open_url`.
  - `fn confirm_lastfm(&mut self, cx)` → `io.spawn(exchange_session)` with
    the pending token; on success, persist `lastfm_session_key` +
    `lastfm_username` to `AppSettings` and toast success; on failure, toast
    failure (`log::warn!` + `Toasts::plain(Outcome::Failed, "toast-lastfm-failed", cx)`
    or equivalent existing helper).
  - `fn disconnect_lastfm(&mut self, cx)` → clear the settings fields.
  - `fn set_listenbrainz_token(&mut self, token: String, cx)` → validates via
    `io.spawn(validate_token)`; on success persists `listenbrainz_token` and
    toasts; on failure toasts and does not persist.
  - `fn disconnect_listenbrainz(&mut self, cx)` → clear the settings field.

Added to `Sonora` (`crates/state/src/lib.rs`) as `pub scrobbling: Entity<Scrobbling>`,
constructed in `init` next to `history`.

## Settings persistence

`crates/state/src/settings.rs`, on `Values` (top-level, alongside `shuffle`/`radio`):

```rust
lastfm_session_key: String,   // empty = disconnected
lastfm_username: String,      // cosmetic, empty when disconnected
listenbrainz_token: String,   // empty = disconnected
```

Defaults: all empty strings. Standard accessors/setters on `AppSettings`
(`lastfm_session_key()`, `set_lastfm_session_key()`, etc.) following the
existing pattern in that file — debounced persistence is already handled by
`AppSettings`'s existing save path.

## UI

`crates/views/src/screens/settings.rs` — new `scrobbling_row(s)` alongside
`accounts_row`. Two rows in a new "Scrobbling" section:

- **Last.fm**: disconnected state shows a "Connect Last.fm" button
  (`cx.open_url` to the URL from `connect_lastfm`, then reveals an
  "I've authorized" button that calls `confirm_lastfm`). Connected state
  shows the stored username + a "Disconnect" button.
- **ListenBrainz**: disconnected state shows an `Input` field (paste token)
  + "Save" button calling `set_listenbrainz_token`. Connected state shows
  "Connected" + "Disconnect" button.

Both rows read `Sonora::global(cx).scrobbling` the same way other settings
rows read `session`/`library`.

## i18n

New keys under the `settings-` scope: `settings-scrobbling`,
`settings-scrobbling-detail`, `settings-lastfm-connect`,
`settings-lastfm-confirm`, `settings-lastfm-disconnect`,
`settings-listenbrainz-token-hint`, `settings-listenbrainz-save`,
`settings-listenbrainz-disconnect`, plus toast keys
(`toast-lastfm-connected`, `toast-lastfm-failed`,
`toast-listenbrainz-connected`, `toast-listenbrainz-failed`). Added to
`assets/i18n/en-US/main.ftl`; other locales may lag per the i18n fallback
rule.

## Error handling

- Background `now_playing`/`scrobble` failures during playback: `log::warn!`
  only, prefixed `"scrobble: "`, no toast — matches `History`'s handling of
  save failures.
- Foreground connect/save actions (`confirm_lastfm`, `set_listenbrainz_token`):
  toast on both success and failure, since the user is actively waiting on
  the result.

## Testing

No new automated tests planned — per project convention, tests are added
only when requested. The Last.fm `api_sig` computation and the threshold
calculation (`min(duration/2, 4min)`) are pure functions and would be
straightforward to unit-test if asked for later.
