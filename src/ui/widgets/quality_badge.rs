//! Text badges for quality and explicit content (docs/design.md §1.6, §4.5).
//! Meaning is carried by the text, never by colour alone.

use gtk::prelude::*;

use crate::i18n::tr;
use crate::tidal::models::{Album, MediaMetadata};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeKind {
    HiRes,
    Lossless,
    Explicit,
}

impl BadgeKind {
    fn text(self) -> &'static str {
        match self {
            Self::HiRes => "HI-RES",
            Self::Lossless => "LOSSLESS",
            Self::Explicit => "E",
        }
    }

    /// Tooltip and accessible name.
    pub fn description(self) -> &'static str {
        match self {
            Self::HiRes => tr("Hi-Res Lossless"),
            Self::Lossless => tr("Lossless"),
            Self::Explicit => tr("Explicit"),
        }
    }

    fn class(self) -> Option<&'static str> {
        match self {
            Self::HiRes => Some("hires"),
            Self::Lossless => None,
            Self::Explicit => Some("explicit"),
        }
    }
}

/// `QualityBadge::new(kind)` of the design: a styled label.
pub struct QualityBadge;

impl QualityBadge {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(kind: BadgeKind) -> gtk::Label {
        let label = gtk::Label::builder()
            .label(kind.text())
            .valign(gtk::Align::Center)
            .tooltip_text(kind.description())
            .css_classes(["badge"])
            .build();
        if let Some(class) = kind.class() {
            label.add_css_class(class);
        }
        label.update_property(&[gtk::accessible::Property::Label(kind.description())]);
        label
    }
}

fn has_tag(metadata: &MediaMetadata, tag: &str) -> bool {
    metadata.tags.iter().any(|t| t == tag)
}

/// The badges of an album header: HI-RES and/or LOSSLESS, then E.
pub fn album_badges(album: &Album) -> Vec<BadgeKind> {
    let mut badges = Vec::new();
    if has_tag(&album.media_metadata, "HIRES_LOSSLESS") {
        badges.push(BadgeKind::HiRes);
    }
    if has_tag(&album.media_metadata, "LOSSLESS")
        || matches!(
            album.audio_quality.as_deref(),
            Some("LOSSLESS" | "HI_RES" | "HI_RES_LOSSLESS")
        )
    {
        badges.push(BadgeKind::Lossless);
    }
    if album.explicit {
        badges.push(BadgeKind::Explicit);
    }
    badges
}
