use std::collections::BTreeMap;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use serde::Deserialize;

use crate::{Scrobbler, Track};

const ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";

pub struct LastfmClient {
    http: reqwest::Client,
    api_key: String,
    api_secret: String,
    session_key: String,
}

impl LastfmClient {
    pub fn new(api_key: String, api_secret: String, session_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            api_secret,
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

fn sign(params: &BTreeMap<&'static str, String>, api_secret: &str) -> String {
    let mut signed = String::new();
    for (key, value) in params {
        signed.push_str(key);
        signed.push_str(value);
    }
    signed.push_str(api_secret);
    format!("{:x}", md5::compute(signed))
}

async fn signed_get<T: for<'de> Deserialize<'de>>(
    params: BTreeMap<&'static str, String>,
    api_secret: &str,
) -> Result<T> {
    let api_sig = sign(&params, api_secret);
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

async fn signed_post(
    http: &reqwest::Client,
    params: BTreeMap<&'static str, String>,
    api_secret: &str,
) -> Result<()> {
    let api_sig = sign(&params, api_secret);
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

pub async fn request_token(api_key: &str, api_secret: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Response {
        token: String,
    }
    let mut params = BTreeMap::new();
    params.insert("method", "auth.getToken".to_owned());
    params.insert("api_key", api_key.to_owned());
    let response: Response = signed_get(params, api_secret).await?;
    Ok(response.token)
}

pub fn auth_url(api_key: &str, token: &str) -> String {
    format!("https://www.last.fm/api/auth/?api_key={api_key}&token={token}")
}

pub async fn exchange_session(
    api_key: &str,
    api_secret: &str,
    token: &str,
) -> Result<(String, String)> {
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
    params.insert("api_key", api_key.to_owned());
    params.insert("token", token.to_owned());
    let response: Response = signed_get(params, api_secret).await?;
    Ok((response.session.key, response.session.name))
}

#[async_trait]
impl Scrobbler for LastfmClient {
    async fn now_playing(&self, track: &Track) -> Result<()> {
        let mut params = BTreeMap::new();
        params.insert("method", "track.updateNowPlaying".to_owned());
        params.insert("api_key", self.api_key.clone());
        params.insert("sk", self.session_key.clone());
        params.insert("artist", artist_of(track));
        params.insert("track", track.name.clone());
        params.insert("album", track.album.clone());
        signed_post(&self.http, params, &self.api_secret).await
    }

    async fn scrobble(&self, track: &Track, started_at: SystemTime) -> Result<()> {
        let timestamp = started_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let mut params = BTreeMap::new();
        params.insert("method", "track.scrobble".to_owned());
        params.insert("api_key", self.api_key.clone());
        params.insert("sk", self.session_key.clone());
        params.insert("artist", artist_of(track));
        params.insert("track", track.name.clone());
        params.insert("album", track.album.clone());
        params.insert("timestamp", timestamp);
        signed_post(&self.http, params, &self.api_secret).await
    }
}
