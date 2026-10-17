//! TIDAL's JSON objects. Every field has a default, so objects with missing
//! or null fields still load; lists skip items that do not parse at all.

use std::collections::HashMap;

use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Deserialises a list, dropping (and logging) items that do not parse.
pub fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let values = Option::<Vec<Value>>::deserialize(deserializer)?.unwrap_or_default();
    Ok(values
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(item) => Some(item),
            Err(err) => {
                log::debug!("skipping an item that does not parse: {err}");
                None
            }
        })
        .collect())
}

/// `null` becomes the default value.
pub fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A v1 page of results.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", bound = "T: DeserializeOwned")]
pub struct Page<T> {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub items: Vec<T>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: usize,
    #[serde(default, rename = "totalNumberOfItems")]
    pub total: usize,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            offset: 0,
            limit: 0,
            total: 0,
        }
    }
}

/// Favourites and mix/playlist items wrap the object with extra data.
#[derive(Debug, Deserialize)]
#[serde(bound = "T: DeserializeOwned")]
pub struct Wrapped<T> {
    pub item: T,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArtistRef {
    pub id: u64,
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    pub picture: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlbumRef {
    pub id: u64,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    pub cover: Option<String>,
    pub vibrant_color: Option<String>,
    pub release_date: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaMetadata {
    #[serde(deserialize_with = "nullable")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Track {
    pub id: u64,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    pub version: Option<String>,
    pub duration: u32,
    pub track_number: u32,
    pub volume_number: u32,
    pub explicit: bool,
    #[serde(deserialize_with = "lenient_vec")]
    pub artists: Vec<ArtistRef>,
    #[serde(deserialize_with = "nullable")]
    pub album: AlbumRef,
    pub audio_quality: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub media_metadata: MediaMetadata,
    #[serde(default = "yes")]
    pub stream_ready: bool,
    #[serde(default = "yes")]
    pub allow_streaming: bool,
    pub popularity: u32,
    pub isrc: Option<String>,
    pub url: Option<String>,
    pub replay_gain: Option<f64>,
    pub peak: Option<f64>,
    pub date_added: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Album {
    pub id: u64,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    pub version: Option<String>,
    pub cover: Option<String>,
    pub vibrant_color: Option<String>,
    pub video_cover: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub artists: Vec<ArtistRef>,
    pub release_date: Option<String>,
    pub duration: u32,
    pub number_of_tracks: u32,
    pub number_of_volumes: u32,
    pub explicit: bool,
    pub audio_quality: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub media_metadata: MediaMetadata,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub copyright: Option<String>,
    pub url: Option<String>,
    pub popularity: u32,
    pub upc: Option<String>,
    #[serde(default = "yes")]
    pub stream_ready: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Artist {
    pub id: u64,
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    pub picture: Option<String>,
    pub selected_album_cover_fallback: Option<String>,
    pub popularity: u32,
    pub url: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub mixes: HashMap<String, String>,
    #[serde(deserialize_with = "nullable")]
    pub artist_types: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Creator {
    pub id: Option<u64>,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Playlist {
    #[serde(deserialize_with = "nullable")]
    pub uuid: String,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    #[serde(deserialize_with = "nullable")]
    pub description: String,
    #[serde(deserialize_with = "nullable")]
    pub creator: Creator,
    pub number_of_tracks: u32,
    pub duration: u32,
    pub image: Option<String>,
    pub square_image: Option<String>,
    pub public_playlist: bool,
    pub created: Option<String>,
    pub last_updated: Option<String>,
    pub last_item_added_at: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub url: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub promoted_artists: Vec<ArtistRef>,
}

/// A mix as v1 (`mixes`, pages) describes it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MixV1 {
    #[serde(deserialize_with = "nullable")]
    pub id: String,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    #[serde(deserialize_with = "nullable")]
    pub sub_title: String,
    pub mix_type: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub images: HashMap<String, MixImageV1>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MixImageV1 {
    pub url: String,
    pub width: u32,
}

/// A mix as the v2 feed and favourites describe it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MixV2 {
    #[serde(deserialize_with = "nullable")]
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub title_text_info: TextInfo,
    #[serde(deserialize_with = "nullable")]
    pub subtitle_text_info: TextInfo,
    #[serde(deserialize_with = "nullable")]
    pub short_subtitle_text_info: TextInfo,
    #[serde(deserialize_with = "lenient_vec")]
    pub mix_images: Vec<SizedImage>,
    #[serde(deserialize_with = "lenient_vec")]
    pub detail_mix_images: Vec<SizedImage>,
    // Older v2 shapes.
    pub title: Option<String>,
    pub sub_title: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TextInfo {
    pub text: Option<String>,
    pub color: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SizedImage {
    pub size: Option<String>,
    pub url: String,
    pub width: u32,
}

/// The mix shape the UI uses, from either API version.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mix {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// Full image URL (mix artwork is not on the cover image server).
    pub image: Option<String>,
    pub kind: Option<String>,
}

impl From<MixV1> for Mix {
    fn from(mix: MixV1) -> Self {
        let image = mix
            .images
            .get("MEDIUM")
            .or_else(|| mix.images.get("SMALL"))
            .or_else(|| mix.images.values().next())
            .map(|image| image.url.clone())
            .filter(|url| !url.is_empty());
        Self {
            id: mix.id,
            title: mix.title,
            subtitle: mix.sub_title,
            image,
            kind: mix.mix_type,
        }
    }
}

impl From<MixV2> for Mix {
    fn from(mix: MixV2) -> Self {
        let images = if mix.mix_images.is_empty() {
            &mix.detail_mix_images
        } else {
            &mix.mix_images
        };
        let image = images
            .iter()
            .find(|image| image.size.as_deref() == Some("MEDIUM"))
            .or_else(|| images.first())
            .map(|image| image.url.clone())
            .filter(|url| !url.is_empty());
        Self {
            id: mix.id,
            title: mix
                .title_text_info
                .text
                .or(mix.title)
                .unwrap_or_default(),
            subtitle: mix
                .subtitle_text_info
                .text
                .or(mix.sub_title)
                .or(mix.short_subtitle_text_info.text)
                .unwrap_or_default(),
            image,
            kind: mix.kind,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Lyrics {
    pub lyrics: Option<String>,
    /// LRC-style synced lines (`[mm:ss.xx] text`).
    pub subtitles: Option<String>,
    pub lyrics_provider: Option<String>,
    #[serde(deserialize_with = "flexible_bool")]
    pub is_right_to_left: bool,
}

/// TIDAL sends some booleans as `"False"`/`"True"` strings.
fn flexible_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Bool(b) => b,
        Value::String(s) => s.eq_ignore_ascii_case("true"),
        Value::Number(n) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    })
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaybackInfo {
    pub track_id: u64,
    pub audio_quality: Option<String>,
    pub audio_mode: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub manifest_mime_type: String,
    #[serde(deserialize_with = "nullable")]
    pub manifest: String,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
    pub track_replay_gain: Option<f64>,
    pub track_peak_amplitude: Option<f64>,
    pub album_replay_gain: Option<f64>,
    pub album_peak_amplitude: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchResults {
    #[serde(deserialize_with = "nullable")]
    pub artists: Page<Artist>,
    #[serde(deserialize_with = "nullable")]
    pub albums: Page<Album>,
    #[serde(deserialize_with = "nullable")]
    pub playlists: Page<Playlist>,
    #[serde(deserialize_with = "nullable")]
    pub tracks: Page<Track>,
    pub top_hit: Option<TopHit>,
}

#[derive(Debug, Deserialize)]
pub struct TopHit {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Value,
}

/// `favorites/ids`: every favourite id by type.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "UPPERCASE", default)]
pub struct FavoriteIds {
    #[serde(deserialize_with = "nullable")]
    pub track: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub album: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub artist: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    pub playlist: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArtistBio {
    #[serde(deserialize_with = "nullable")]
    pub text: String,
    #[serde(deserialize_with = "nullable")]
    pub summary: String,
    pub source: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlbumReview {
    #[serde(deserialize_with = "nullable")]
    pub text: String,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Credit {
    #[serde(rename = "type", deserialize_with = "nullable")]
    pub role: String,
    #[serde(deserialize_with = "lenient_vec")]
    pub contributors: Vec<Contributor>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Contributor {
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    pub id: Option<u64>,
}

/// v2 cursor-paged list (`favorites/mixes`, `my-collection/...`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", bound = "T: DeserializeOwned")]
pub struct CursorPage<T> {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub items: Vec<T>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bad_item_is_skipped() {
        let page: Page<Track> = serde_json::from_str(
            r#"{"items":[{"id":1,"title":"a","album":null},"oops",{"id":2}],"totalNumberOfItems":3}"#,
        )
        .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 3);
        assert!(page.items[0].stream_ready);
    }

    #[test]
    fn lyrics_string_bool() {
        let lyrics: Lyrics = serde_json::from_str(r#"{"isRightToLeft":"False"}"#).unwrap();
        assert!(!lyrics.is_right_to_left);
    }

    #[test]
    fn v2_mix() {
        let mix: MixV2 = serde_json::from_str(
            r#"{"id":"x","titleTextInfo":{"text":"Mix"},"subtitleTextInfo":{"text":"Sub"},
                "mixImages":[{"size":"SMALL","url":"s","width":320},{"size":"MEDIUM","url":"m","width":640}]}"#,
        )
        .unwrap();
        let mix = Mix::from(mix);
        assert_eq!(mix.title, "Mix");
        assert_eq!(mix.image.as_deref(), Some("m"));
    }
}
