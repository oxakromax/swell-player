//! TIDAL's editorial pages (v1 `pages/...`) and the personalised v2 home
//! feed, reduced to one shape: sections of items.

use serde_json::Value;

use super::models::*;

/// Where a link leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
    Track(u64),
    /// A v1 page path such as `pages/genre_hip_hop`.
    Page(String),
    /// A collection view (`tidal://my-collection/...`).
    Collection(String),
    Url(String),
}

impl Target {
    /// Parses `tidal://album/1`, `https://tidal.com/browse/album/1`,
    /// `https://listen.tidal.com/album/1/track/2` and the like.
    pub fn from_url(url: &str) -> Option<Self> {
        let rest = url
            .strip_prefix("tidal://")
            .or_else(|| {
                let without_scheme = url.split_once("://")?.1;
                let (host, path) = without_scheme.split_once('/')?;
                host.ends_with("tidal.com").then_some(path)
            })?
            .trim_start_matches("browse/")
            .trim_end_matches('/');
        let rest = rest.split(['?', '#']).next()?;
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        // The last kind/id pair wins: album/1/track/2 is the track.
        let pair = parts
            .windows(2)
            .rev()
            .find(|w| matches!(w[0], "album" | "artist" | "playlist" | "mix" | "track"));
        if let Some([kind, id]) = pair.map(|w| [w[0], w[1]]) {
            return match kind {
                "album" => id.parse().ok().map(Self::Album),
                "artist" => id.parse().ok().map(Self::Artist),
                "track" => id.parse().ok().map(Self::Track),
                "playlist" => Some(Self::Playlist(id.to_owned())),
                "mix" => Some(Self::Mix(id.to_owned())),
                _ => None,
            };
        }
        if let Some(view) = rest.strip_prefix("my-collection/") {
            return Some(Self::Collection(view.to_owned()));
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct Link {
    pub title: String,
    pub subtitle: Option<String>,
    /// Image id on TIDAL's image server, if any.
    pub image: Option<String>,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub enum Item {
    Track(Box<Track>),
    Album(Box<Album>),
    Artist(Box<Artist>),
    Playlist(Box<Playlist>),
    Mix(Box<Mix>),
    Link(Box<Link>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    /// A short track list.
    Tracks,
    /// A row of cards.
    Cards,
    /// Category chips (genres, moods).
    Links,
    /// Large promoted items.
    Featured,
    /// Quick-access tiles at the top of Home.
    Shortcuts,
}

/// Where to get the rest of a section.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum More {
    /// Page with `offset`/`limit` through a v1 `dataApiPath`.
    Data(String),
    /// A v2 view-all path.
    ViewAll(String),
    /// A v1 page showing just this module.
    Page(String),
}

#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: SectionKind,
    pub items: Vec<Item>,
    /// Total number of items TIDAL has for it (≥ `items.len()`).
    pub total: usize,
    pub more: Option<More>,
}

#[derive(Clone, Debug)]
pub enum Header {
    Album {
        album: Box<Album>,
        review: Option<String>,
        credits: Vec<Credit>,
    },
    Artist {
        artist: Box<Artist>,
        bio: Option<String>,
    },
    Mix(Box<Mix>),
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub title: String,
    pub header: Option<Header>,
    pub sections: Vec<Section>,
}

impl Page {
    /// The first section of tracks (an album's or mix's track list).
    pub fn tracks(&self) -> Option<&Section> {
        self.sections.iter().find(|s| s.kind == SectionKind::Tracks)
    }
}

/// The personalised home feed.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub sections: Vec<Section>,
    pub cursor: Option<String>,
}

fn str_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

/// One item of a list whose element type is given by `kind` (`ALBUM`,
/// `track`, `MIX`...).
fn item_of(kind: &str, value: &Value) -> Option<Item> {
    Some(match kind.to_ascii_uppercase().as_str() {
        "TRACK" => Item::Track(Box::new(parse(value)?)),
        "ALBUM" => Item::Album(Box::new(parse(value)?)),
        "ARTIST" => Item::Artist(Box::new(parse(value)?)),
        "PLAYLIST" => Item::Playlist(Box::new(parse(value)?)),
        "MIX" => {
            // v1 mixes have `title`, v2 ones `titleTextInfo`.
            let mix = if value.get("titleTextInfo").is_some() {
                Mix::from(parse::<MixV2>(value)?)
            } else {
                Mix::from(parse::<MixV1>(value)?)
            };
            Item::Mix(Box::new(mix))
        }
        _ => return None,
    })
}

fn link_of(value: &Value) -> Option<Item> {
    let title = str_of(value, "title")?;
    let target = if let Some(path) = str_of(value, "apiPath") {
        // Swell does not play videos.
        if path.contains("video") {
            return None;
        }
        Target::Page(path)
    } else {
        Target::from_url(&str_of(value, "url")?)?
    };
    // Page links carry icon names in imageId ("hiphop"), not image ids.
    let image = str_of(value, "imageId").filter(|id| id.len() == 36);
    Some(Item::Link(Box::new(Link {
        title,
        subtitle: None,
        image,
        target,
    })))
}

fn promotion_of(value: &Value) -> Option<Item> {
    let id = str_of(value, "artifactId")?;
    let target = match str_of(value, "type")?.as_str() {
        "ALBUM" => Target::Album(id.parse().ok()?),
        "ARTIST" => Target::Artist(id.parse().ok()?),
        "PLAYLIST" => Target::Playlist(id),
        "MIX" => Target::Mix(id),
        "TRACK" => Target::Track(id.parse().ok()?),
        "EXTURL" => Target::Url(id),
        _ => return None,
    };
    Some(Item::Link(Box::new(Link {
        title: str_of(value, "shortHeader").or_else(|| str_of(value, "header"))?,
        subtitle: str_of(value, "shortSubHeader").or_else(|| str_of(value, "header")),
        image: str_of(value, "imageId"),
        target,
    })))
}

/// The element type of a v1 module's list.
fn module_item_kind(module_type: &str) -> Option<&'static str> {
    Some(match module_type {
        "TRACK_LIST" | "ALBUM_ITEMS" => "TRACK",
        "ALBUM_LIST" => "ALBUM",
        "ARTIST_LIST" => "ARTIST",
        "PLAYLIST_LIST" => "PLAYLIST",
        "MIX_LIST" => "MIX",
        "MIXED_TYPES_LIST" => "",
        _ => return None,
    })
}

fn items_of(kind: &str, values: &[Value]) -> Vec<Item> {
    values
        .iter()
        .filter_map(|value| {
            // Mixed lists and album items wrap each object with its type.
            if let (Some(item), Some(item_kind)) =
                (value.get("item"), value.get("type").and_then(Value::as_str))
            {
                return item_of(item_kind, item);
            }
            item_of(kind, value)
        })
        .collect()
}

fn section_kind(module_type: &str) -> SectionKind {
    match module_type {
        "TRACK_LIST" | "ALBUM_ITEMS" => SectionKind::Tracks,
        _ => SectionKind::Cards,
    }
}

fn parse_module(module: &Value) -> Option<Section> {
    let module_type = module.get("type")?.as_str()?;
    let title = str_of(module, "title").unwrap_or_default();
    let subtitle = str_of(module, "description");
    match module_type {
        "PAGE_LINKS_CLOUD" | "PAGE_LINKS" => {
            let values = module
                .get("pagedList")
                .and_then(|p| p.get("items"))
                .or_else(|| module.get("items"))?
                .as_array()?;
            let items: Vec<Item> = values.iter().filter_map(link_of).collect();
            // The rest of a long cloud (moods) is a data list of links.
            let paged = module.get("pagedList");
            let total = paged
                .and_then(|p| p.get("totalNumberOfItems"))
                .and_then(Value::as_u64)
                .map_or(items.len(), |t| (t as usize).max(items.len()));
            let more = (total > items.len())
                .then(|| paged.and_then(|p| str_of(p, "dataApiPath")).map(More::Data))
                .flatten();
            Some(Section {
                title,
                subtitle,
                kind: SectionKind::Links,
                total: if more.is_some() { total } else { items.len() },
                items,
                more,
            })
        }
        "FEATURED_PROMOTIONS" | "MULTIPLE_TOP_PROMOTIONS" => {
            let items: Vec<Item> = module
                .get("items")?
                .as_array()?
                .iter()
                .filter_map(promotion_of)
                .collect();
            Some(Section {
                title,
                subtitle,
                kind: SectionKind::Featured,
                total: items.len(),
                items,
                more: None,
            })
        }
        _ => {
            let kind = module_item_kind(module_type)?;
            let paged = module.get("pagedList")?;
            let values = paged.get("items")?.as_array()?;
            let items = items_of(kind, values);
            if items.is_empty() {
                return None;
            }
            let total = paged
                .get("totalNumberOfItems")
                .and_then(Value::as_u64)
                .map_or(items.len(), |t| (t as usize).max(items.len()));
            let more = (total > items.len())
                .then(|| str_of(paged, "dataApiPath").map(More::Data))
                .flatten()
                .or_else(|| {
                    module
                        .get("showMore")
                        .and_then(|m| str_of(m, "apiPath"))
                        .map(More::Page)
                });
            Some(Section {
                title,
                subtitle,
                kind: section_kind(module_type),
                items,
                total,
                more,
            })
        }
    }
}

fn parse_header(module: &Value) -> Option<Header> {
    match module.get("type")?.as_str()? {
        "ALBUM_HEADER" => Some(Header::Album {
            album: Box::new(parse(module.get("album")?)?),
            review: module
                .get("review")
                .and_then(|r| str_of(r, "text"))
                .map(|text| strip_links(&text)),
            credits: module
                .get("credits")
                .and_then(|c| c.get("items"))
                .and_then(parse)
                .unwrap_or_default(),
        }),
        "ARTIST_HEADER" => Some(Header::Artist {
            artist: Box::new(parse(module.get("artist")?)?),
            bio: module
                .get("bio")
                .and_then(|b| str_of(b, "text"))
                .map(|text| strip_links(&text)),
        }),
        "MIX_HEADER" => Some(Header::Mix(Box::new(Mix::from(parse::<MixV1>(
            module.get("mix")?,
        )?)))),
        _ => None,
    }
}

/// Removes TIDAL's `[wimpLink ...]text[/wimpLink]` markup.
pub fn strip_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find(']') {
            Some(end)
                if after.starts_with("[wimpLink")
                    || after.starts_with("[/wimpLink")
                    || after.starts_with("[br") =>
            {
                if after.starts_with("[br") {
                    out.push('\n');
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('[');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out.replace("<br/>", "\n").replace("<br />", "\n")
}

pub fn parse_v1(value: &Value) -> Page {
    let mut page = Page {
        title: str_of(value, "title").unwrap_or_default(),
        ..Page::default()
    };
    let rows = value.get("rows").and_then(Value::as_array);
    for module in rows
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("modules")?.as_array())
        .flatten()
    {
        if page.header.is_none()
            && let Some(header) = parse_header(module)
        {
            page.header = Some(header);
            continue;
        }
        if let Some(section) = parse_module(module) {
            page.sections.push(section);
        }
    }
    page
}

/// A `pages/data/...` answer: a plain paged list of mixed items.
pub fn parse_paged_list(value: &Value) -> Section {
    let values = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    // Data lists don't say their type; guess from each object's shape.
    let items: Vec<Item> = values
        .iter()
        .filter_map(|v| {
            if let (Some(item), Some(kind)) = (v.get("item"), v.get("type").and_then(Value::as_str))
            {
                return item_of(kind, item);
            }
            if v.get("apiPath").is_some() {
                return link_of(v);
            }
            let kind = if v.get("uuid").is_some() {
                "PLAYLIST"
            } else if v.get("name").is_some() {
                "ARTIST"
            } else if v.get("trackNumber").is_some() || v.get("album").is_some() {
                "TRACK"
            } else if v.get("numberOfTracks").is_some() || v.get("cover").is_some() {
                "ALBUM"
            } else if v.get("mixType").is_some() || v.get("titleTextInfo").is_some() {
                "MIX"
            } else {
                return None;
            };
            item_of(kind, v)
        })
        .collect();
    let kind = list_kind(&items);
    Section {
        title: String::new(),
        subtitle: None,
        kind,
        total: value
            .get("totalNumberOfItems")
            .and_then(Value::as_u64)
            .map_or(items.len(), |t| t as usize),
        items,
        more: None,
    }
}

/// How a list of `items` that does not say its type is shown.
fn list_kind(items: &[Item]) -> SectionKind {
    if items.is_empty() {
        SectionKind::Cards
    } else if items.iter().all(|i| matches!(i, Item::Track(_))) {
        SectionKind::Tracks
    } else if items
        .iter()
        .all(|i| matches!(i, Item::Link(link) if matches!(link.target, Target::Page(_))))
    {
        SectionKind::Links
    } else {
        SectionKind::Cards
    }
}

fn feed_items(values: &[Value]) -> Vec<Item> {
    values
        .iter()
        .filter_map(|v| {
            let kind = v.get("type")?.as_str()?;
            let data = v.get("data")?;
            match kind {
                "DEEP_LINK" => {
                    let title = str_of(data, "title")?;
                    let target = Target::from_url(&str_of(data, "url")?)?;
                    Some(Item::Link(Box::new(Link {
                        title,
                        subtitle: None,
                        image: None,
                        target,
                    })))
                }
                _ => item_of(kind, data),
            }
        })
        .collect()
}

pub fn parse_feed(value: &Value) -> Feed {
    let sections = value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|module| {
            let values = module.get("items")?.as_array()?;
            let items = feed_items(values);
            if items.is_empty() {
                return None;
            }
            let kind = match module.get("type").and_then(Value::as_str) {
                Some("SHORTCUT_LIST") => SectionKind::Shortcuts,
                Some("TRACK_LIST") => SectionKind::Tracks,
                _ => SectionKind::Cards,
            };
            let more = str_of(module, "viewAll").map(More::ViewAll);
            Some(Section {
                title: str_of(module, "title").unwrap_or_default(),
                subtitle: str_of(module, "subtitle")
                    .or_else(|| module.get("description").and_then(|d| str_of(d, "text"))),
                kind,
                // The feed does not say how many there are; a module with a
                // "view all" page has at least one more than it shows.
                total: items.len() + usize::from(more.is_some()),
                items,
                more,
            })
        })
        .collect();
    let cursor = value
        .get("page")
        .and_then(|p| str_of(p, "cursor"))
        .filter(|c| !c.is_empty());
    Feed { sections, cursor }
}

pub fn parse_view_all(value: &Value) -> Section {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(|v| feed_items(v))
        .unwrap_or_default();
    let kind = list_kind(&items);
    Section {
        title: str_of(value, "title").unwrap_or_default(),
        subtitle: str_of(value, "subtitle"),
        kind,
        total: items.len(),
        items,
        more: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            Target::from_url("tidal://album/12"),
            Some(Target::Album(12))
        );
        assert_eq!(
            Target::from_url("https://tidal.com/browse/track/5?u"),
            Some(Target::Track(5))
        );
        assert_eq!(
            Target::from_url("https://listen.tidal.com/album/1/track/2"),
            Some(Target::Track(2))
        );
        assert_eq!(
            Target::from_url("tidal://my-collection/tracks"),
            Some(Target::Collection("tracks".into()))
        );
        assert_eq!(
            Target::from_url("https://tidal.com/playlist/ab-cd"),
            Some(Target::Playlist("ab-cd".into()))
        );
        assert_eq!(Target::from_url("https://example.com/album/1"), None);
    }

    #[test]
    fn wimp_links() {
        assert_eq!(
            strip_links(r#"By [wimpLink artistId="1"]Eminem[/wimpLink].[br]Next"#),
            "By Eminem.\nNext"
        );
        assert_eq!(strip_links("a [b] c"), "a [b] c");
    }

    #[test]
    fn module_with_more() {
        let module: Value = serde_json::json!({
            "type": "ALBUM_LIST", "title": "New",
            "pagedList": {"items": [{"id": 1, "title": "A"}], "totalNumberOfItems": 9,
                          "dataApiPath": "pages/data/x"}
        });
        let section = parse_module(&module).unwrap();
        assert_eq!(section.total, 9);
        assert_eq!(section.more, Some(More::Data("pages/data/x".into())));
    }

    #[test]
    fn link_cloud_with_more() {
        let module: Value = serde_json::json!({
            "type": "PAGE_LINKS_CLOUD", "title": "Moods",
            "pagedList": {"items": [
                {"title": "Relax", "apiPath": "pages/mood_relax", "imageId": "relax"},
                {"title": "Videos", "apiPath": "pages/videos"},
                {"title": "", "apiPath": "pages/untitled"}
            ], "totalNumberOfItems": 19, "dataApiPath": "pages/data/moods"}
        });
        let section = parse_module(&module).unwrap();
        assert_eq!(section.kind, SectionKind::Links);
        assert_eq!(section.items.len(), 1);
        let Item::Link(link) = &section.items[0] else {
            panic!("not a link");
        };
        assert_eq!(link.target, Target::Page("pages/mood_relax".into()));
        assert_eq!(link.image, None);
        assert_eq!(section.more, Some(More::Data("pages/data/moods".into())));

        let list = parse_paged_list(&serde_json::json!({
            "items": [{"title": "Relax", "apiPath": "pages/mood_relax"}],
            "totalNumberOfItems": 1
        }));
        assert_eq!(list.kind, SectionKind::Links);
        assert_eq!(list.items.len(), 1);
    }

    #[test]
    fn feed_modules() {
        let feed = parse_feed(&serde_json::json!({
            "page": {"cursor": "next"},
            "items": [
                {"type": "TRACK_LIST", "title": "New", "viewAll": "home/pages/NEW/view-all",
                 "items": [{"type": "TRACK", "data": {"id": 1, "title": "A"}}]},
                {"type": "SHORTCUT_LIST", "title": "Shortcuts",
                 "items": [{"type": "DEEP_LINK", "data": {
                     "title": "My Tracks", "url": "tidal://my-collection/tracks"}}]},
                {"type": "VIDEO_LIST", "title": "Videos",
                 "items": [{"type": "VIDEO", "data": {"id": 2}}]}
            ]
        }));
        assert_eq!(feed.cursor.as_deref(), Some("next"));
        assert_eq!(feed.sections.len(), 2);
        let tracks = &feed.sections[0];
        assert_eq!(tracks.kind, SectionKind::Tracks);
        // More than it shows, so "View All" appears.
        assert!(tracks.total > tracks.items.len());
        assert_eq!(
            tracks.more,
            Some(More::ViewAll("home/pages/NEW/view-all".into()))
        );
        assert_eq!(feed.sections[1].kind, SectionKind::Shortcuts);
    }
}
