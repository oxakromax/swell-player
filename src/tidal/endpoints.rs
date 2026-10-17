//! The API calls Swell makes, grouped by area.

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use super::models::*;
use super::pages::{self, Feed, Page as PageView, Section};
use super::{Base, Call, Client};

/// Largest page TIDAL serves for most v1 lists.
pub const MAX_PAGE: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Favorite {
    Track,
    Album,
    Artist,
    Playlist,
}

impl Favorite {
    fn plural(self) -> &'static str {
        match self {
            Self::Track => "tracks",
            Self::Album => "albums",
            Self::Artist => "artists",
            Self::Playlist => "playlists",
        }
    }

    fn form_key(self) -> &'static str {
        match self {
            Self::Track => "trackIds",
            Self::Album => "albumIds",
            Self::Artist => "artistIds",
            Self::Playlist => "uuids",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlbumFilter {
    Albums,
    EpsAndSingles,
    Compilations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchType {
    Artists,
    Albums,
    Tracks,
    Playlists,
}

impl SearchType {
    fn name(self) -> &'static str {
        match self {
            Self::Artists => "ARTISTS",
            Self::Albums => "ALBUMS",
            Self::Tracks => "TRACKS",
            Self::Playlists => "PLAYLISTS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    High,
    Lossless,
    HiResLossless,
}

impl Quality {
    pub fn api_name(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::High => "HIGH",
            Self::Lossless => "LOSSLESS",
            Self::HiResLossless => "HI_RES_LOSSLESS",
        }
    }
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

impl Client {
    fn uid_path(&self, rest: &str) -> String {
        format!("users/{}/{rest}", self.user_id())
    }

    // ---- Tracks and playback ----

    pub async fn track(&self, id: u64) -> Result<Track> {
        self.get(Call::get(Base::V1, format!("tracks/{id}"))).await
    }

    pub async fn playback_info(&self, id: u64, quality: Quality) -> Result<PlaybackInfo> {
        self.get(
            Call::get(Base::V1, format!("tracks/{id}/playbackinfopostpaywall"))
                .query("audioquality", quality.api_name())
                .query("playbackmode", "STREAM")
                .query("assetpresentation", "FULL"),
        )
        .await
    }

    pub async fn lyrics(&self, id: u64) -> Result<Lyrics> {
        self.get(Call::get(Base::V1, format!("tracks/{id}/lyrics")))
            .await
    }

    /// Id of the track's radio mix.
    pub async fn track_mix(&self, id: u64) -> Result<String> {
        let mix: IdOnly = self
            .get(Call::get(Base::V1, format!("tracks/{id}/mix")))
            .await?;
        Ok(mix.id)
    }

    pub async fn track_credits(&self, id: u64) -> Result<Vec<Credit>> {
        #[derive(Deserialize)]
        struct Credits(#[serde(deserialize_with = "lenient_vec")] Vec<Credit>);
        let credits: Credits = self
            .get(
                Call::get(Base::V1, format!("tracks/{id}/credits"))
                    .query("includeContributors", "true"),
            )
            .await?;
        Ok(credits.0)
    }

    // ---- Pages ----

    /// A v1 page such as `pages/explore` or `pages/album?albumId=1`.
    pub async fn page(&self, path: &str) -> Result<PageView> {
        let value: Value = self.get(Call::get(Base::V1, path)).await?;
        Ok(pages::parse_v1(&value))
    }

    /// More items of a v1 page module (its `dataApiPath`).
    pub async fn page_data(&self, path: &str, offset: usize, limit: usize) -> Result<Section> {
        let value: Value = self
            .get(Call::get(Base::V1, path).page(offset, limit))
            .await?;
        Ok(pages::parse_paged_list(&value))
    }

    pub async fn home(&self, cursor: Option<&str>) -> Result<Feed> {
        let mut call = Call::get(Base::V2, "home/feed/static").query("platform", "WEB");
        if let Some(cursor) = cursor {
            call = call.query("cursor", cursor);
        }
        let value: Value = self.get(call).await?;
        Ok(pages::parse_feed(&value))
    }

    /// A v2 "view all" page (`viewAll` of a feed module).
    pub async fn view_all(&self, path: &str) -> Result<Section> {
        let value: Value = self
            .get(Call::get(Base::V2, path).query("platform", "WEB"))
            .await?;
        Ok(pages::parse_view_all(&value))
    }

    // ---- Albums ----

    pub async fn album(&self, id: u64) -> Result<Album> {
        self.get(Call::get(Base::V1, format!("albums/{id}"))).await
    }

    pub async fn album_page(&self, id: u64) -> Result<PageView> {
        self.page(&format!("pages/album?albumId={id}")).await
    }

    pub async fn album_tracks(&self, id: u64, offset: usize, limit: usize) -> Result<Page<Track>> {
        self.get(Call::get(Base::V1, format!("albums/{id}/tracks")).page(offset, limit))
            .await
    }

    pub async fn album_credits(&self, id: u64) -> Result<Vec<Credit>> {
        #[derive(Deserialize)]
        struct Credits(#[serde(deserialize_with = "lenient_vec")] Vec<Credit>);
        let credits: Credits = self
            .get(
                Call::get(Base::V1, format!("albums/{id}/credits"))
                    .query("includeContributors", "true"),
            )
            .await?;
        Ok(credits.0)
    }

    // ---- Artists ----

    pub async fn artist(&self, id: u64) -> Result<Artist> {
        self.get(Call::get(Base::V1, format!("artists/{id}"))).await
    }

    pub async fn artist_page(&self, id: u64) -> Result<PageView> {
        self.page(&format!("pages/artist?artistId={id}")).await
    }

    pub async fn artist_top_tracks(
        &self,
        id: u64,
        offset: usize,
        limit: usize,
    ) -> Result<Page<Track>> {
        self.get(Call::get(Base::V1, format!("artists/{id}/toptracks")).page(offset, limit))
            .await
    }

    pub async fn artist_albums(
        &self,
        id: u64,
        filter: AlbumFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Page<Album>> {
        let mut call = Call::get(Base::V1, format!("artists/{id}/albums")).page(offset, limit);
        match filter {
            AlbumFilter::Albums => {}
            AlbumFilter::EpsAndSingles => call = call.query("filter", "EPSANDSINGLES"),
            AlbumFilter::Compilations => call = call.query("filter", "COMPILATIONS"),
        }
        self.get(call).await
    }

    pub async fn similar_artists(&self, id: u64) -> Result<Page<Artist>> {
        self.get(Call::get(Base::V1, format!("artists/{id}/similar")).query("limit", 50))
            .await
    }

    pub async fn artist_bio(&self, id: u64) -> Result<ArtistBio> {
        self.get(Call::get(Base::V1, format!("artists/{id}/bio")))
            .await
    }

    /// Id of the artist's radio mix.
    pub async fn artist_mix(&self, id: u64) -> Result<String> {
        let mix: IdOnly = self
            .get(Call::get(Base::V1, format!("artists/{id}/mix")))
            .await?;
        Ok(mix.id)
    }

    // ---- Playlists and mixes ----

    /// The playlist and its ETag (needed to edit it).
    pub async fn playlist(&self, uuid: &str) -> Result<(Playlist, Option<String>)> {
        let response = self
            .send(&Call::get(Base::V1, format!("playlists/{uuid}")))
            .await?;
        Ok((response.json()?, response.etag()))
    }

    pub async fn playlist_tracks(
        &self,
        uuid: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Page<Track>> {
        let page: Page<Wrapped<Value>> = self
            .get(Call::get(Base::V1, format!("playlists/{uuid}/items")).page(offset, limit))
            .await?;
        Ok(tracks_of(page))
    }

    pub async fn mix_tracks(&self, id: &str, offset: usize, limit: usize) -> Result<Page<Track>> {
        let page: Page<Wrapped<Value>> = self
            .get(Call::get(Base::V1, format!("mixes/{id}/items")).page(offset, limit))
            .await?;
        Ok(tracks_of(page))
    }

    pub async fn mix_page(&self, id: &str) -> Result<PageView> {
        self.page(&format!("pages/mix?mixId={id}")).await
    }

    pub async fn create_playlist(&self, title: &str, description: &str) -> Result<Playlist> {
        self.send(
            &Call::post(Base::V1, self.uid_path("playlists"))
                .form("title", title)
                .form("description", description),
        )
        .await?
        .json()
    }

    pub async fn edit_playlist(&self, uuid: &str, title: &str, description: &str) -> Result<()> {
        self.write(
            Call::post(Base::V1, format!("playlists/{uuid}"))
                .form("title", title)
                .form("description", description),
        )
        .await?;
        Ok(())
    }

    pub async fn set_playlist_public(&self, uuid: &str, public: bool) -> Result<()> {
        let action = if public { "set-public" } else { "set-private" };
        self.write(Call::put(Base::V2, format!("playlists/{uuid}/{action}")))
            .await?;
        Ok(())
    }

    pub async fn delete_playlist(&self, uuid: &str) -> Result<()> {
        self.write(Call::delete(Base::V1, format!("playlists/{uuid}")))
            .await?;
        Ok(())
    }

    /// Appends tracks; duplicates are skipped.
    pub async fn add_to_playlist(&self, uuid: &str, track_ids: &[u64]) -> Result<()> {
        let (_, etag) = self.playlist(uuid).await?;
        let ids = join(track_ids);
        let mut call = Call::post(Base::V1, format!("playlists/{uuid}/items"))
            .form("trackIds", ids)
            .form("onDupes", "SKIP");
        if let Some(etag) = etag {
            call = call.header("If-None-Match", etag);
        }
        self.write(call).await?;
        Ok(())
    }

    /// Removes the items at `indices` (positions in the playlist).
    pub async fn remove_from_playlist(&self, uuid: &str, indices: &[usize]) -> Result<()> {
        let (_, etag) = self.playlist(uuid).await?;
        let mut call = Call::delete(
            Base::V1,
            format!("playlists/{uuid}/items/{}", join(indices)),
        );
        if let Some(etag) = etag {
            call = call.header("If-None-Match", etag);
        }
        self.write(call).await?;
        Ok(())
    }

    /// Moves the item at `from` so it ends up at `to`.
    pub async fn move_in_playlist(&self, uuid: &str, from: usize, to: usize) -> Result<()> {
        let (_, etag) = self.playlist(uuid).await?;
        let mut call =
            Call::post(Base::V1, format!("playlists/{uuid}/items/{from}")).form("toIndex", to);
        if let Some(etag) = etag {
            call = call.header("If-None-Match", etag);
        }
        self.write(call).await?;
        Ok(())
    }

    /// Playlists the user created and the ones they favourited.
    pub async fn user_playlists(&self, offset: usize, limit: usize) -> Result<Page<Playlist>> {
        #[derive(Deserialize)]
        struct Entry {
            playlist: Playlist,
        }
        let page: Page<Entry> = self
            .get(
                Call::get(Base::V1, self.uid_path("playlistsAndFavoritePlaylists"))
                    .page(offset, limit)
                    .query("order", "DATE")
                    .query("orderDirection", "DESC"),
            )
            .await?;
        Ok(Page {
            items: page.items.into_iter().map(|entry| entry.playlist).collect(),
            offset: page.offset,
            limit: page.limit,
            total: page.total,
        })
    }

    // ---- Favourites ----

    pub async fn favorite_ids(&self) -> Result<FavoriteIds> {
        self.get(Call::get(Base::V1, self.uid_path("favorites/ids")))
            .await
    }

    pub async fn favorite_tracks(&self, offset: usize, limit: usize) -> Result<Page<Track>> {
        let page: Page<Wrapped<Track>> = self.favorites(Favorite::Track, offset, limit).await?;
        Ok(unwrap_page(page, |created, mut track: Track| {
            track.date_added = created;
            track
        }))
    }

    pub async fn favorite_albums(&self, offset: usize, limit: usize) -> Result<Page<Album>> {
        let page: Page<Wrapped<Album>> = self.favorites(Favorite::Album, offset, limit).await?;
        Ok(unwrap_page(page, |_, album| album))
    }

    pub async fn favorite_artists(&self, offset: usize, limit: usize) -> Result<Page<Artist>> {
        let page: Page<Wrapped<Artist>> = self.favorites(Favorite::Artist, offset, limit).await?;
        Ok(unwrap_page(page, |_, artist| artist))
    }

    async fn favorites<T: serde::de::DeserializeOwned>(
        &self,
        kind: Favorite,
        offset: usize,
        limit: usize,
    ) -> Result<Page<Wrapped<T>>> {
        self.get(
            Call::get(Base::V1, self.uid_path(&format!("favorites/{}", kind.plural())))
                .page(offset, limit)
                .query("order", "DATE")
                .query("orderDirection", "DESC"),
        )
        .await
    }

    pub async fn favorite_mixes(&self, cursor: Option<&str>) -> Result<CursorPage<Mix>> {
        let mut call = Call::get(Base::V2, "favorites/mixes").query("limit", 50);
        if let Some(cursor) = cursor {
            call = call.query("cursor", cursor);
        }
        let page: CursorPage<MixV2> = self.get(call).await?;
        Ok(CursorPage {
            items: page.items.into_iter().map(Mix::from).collect(),
            cursor: page.cursor.filter(|c| !c.is_empty()),
        })
    }

    pub async fn add_favorite(&self, kind: Favorite, id: &str) -> Result<()> {
        self.write(
            Call::post(Base::V1, self.uid_path(&format!("favorites/{}", kind.plural())))
                .form(kind.form_key(), id),
        )
        .await?;
        Ok(())
    }

    pub async fn remove_favorite(&self, kind: Favorite, id: &str) -> Result<()> {
        self.write(Call::delete(
            Base::V1,
            self.uid_path(&format!("favorites/{}/{id}", kind.plural())),
        ))
        .await?;
        Ok(())
    }

    pub async fn set_mix_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        let action = if favorite { "add" } else { "remove" };
        self.write(
            Call::put(Base::V2, format!("favorites/mixes/{action}"))
                .query("mixIds", id)
                .query("onArtifactNotFound", "FAIL"),
        )
        .await?;
        Ok(())
    }

    // ---- Search ----

    pub async fn search(
        &self,
        query: &str,
        types: &[SearchType],
        offset: usize,
        limit: usize,
    ) -> Result<SearchResults> {
        let types = types
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(",");
        self.get(
            Call::get(Base::V1, "search")
                .query("query", query)
                .query("types", types)
                .query("includeContributors", "true")
                .page(offset, limit),
        )
        .await
    }

    // ---- Blocking ----

    /// Every blocked artist (fetched 100 at a time).
    pub async fn blocked_artists(&self) -> Result<Page<Artist>> {
        let path = self.uid_path("blocks/artists");
        let page: Page<Wrapped<Artist>> = self.get(Call::get(Base::V1, &path).page(0, MAX_PAGE)).await?;
        let mut all = unwrap_page(page, |_, artist| artist);
        // Advance by the page size, not by the items kept: lenient parsing
        // may drop malformed ones. Bounded in case the total is wrong.
        let mut offset = MAX_PAGE;
        while offset < all.total && offset < 100 * MAX_PAGE {
            let page: Page<Wrapped<Artist>> =
                self.get(Call::get(Base::V1, &path).page(offset, MAX_PAGE)).await?;
            if page.items.is_empty() {
                break;
            }
            all.items.extend(unwrap_page(page, |_, artist| artist).items);
            offset += MAX_PAGE;
        }
        all.limit = all.items.len();
        Ok(all)
    }

    pub async fn set_artist_blocked(&self, id: u64, blocked: bool) -> Result<()> {
        let call = if blocked {
            Call::post(Base::V1, self.uid_path("blocks/artists")).form("artistId", id)
        } else {
            Call::delete(Base::V1, self.uid_path(&format!("blocks/artists/{id}")))
        };
        self.write(call).await?;
        Ok(())
    }
}

fn join<T: ToString>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn unwrap_page<W, T>(page: Page<Wrapped<W>>, f: impl Fn(Option<String>, W) -> T) -> Page<T> {
    Page {
        items: page
            .items
            .into_iter()
            .map(|wrapped| f(wrapped.created, wrapped.item))
            .collect(),
        offset: page.offset,
        limit: page.limit,
        total: page.total,
    }
}

/// Keeps the tracks of a mixed track/video list.
fn tracks_of(page: Page<Wrapped<Value>>) -> Page<Track> {
    Page {
        items: page
            .items
            .into_iter()
            .filter(|w| w.kind.as_deref().is_none_or(|k| k.eq_ignore_ascii_case("track")))
            .filter_map(|w| {
                let mut track: Track = serde_json::from_value(w.item).ok()?;
                track.date_added = w.created.or(track.date_added);
                Some(track)
            })
            .collect(),
        offset: page.offset,
        limit: page.limit,
        total: page.total,
    }
}
