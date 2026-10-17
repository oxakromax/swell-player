//! What is being played, described for people ("24-bit/96 kHz FLAC") and as
//! data for the quality popover, plus the ReplayGain and quality arithmetic.
//! Pure functions, unit-tested.

use crate::i18n::tr;
use crate::settings::{Normalization, QualityPreference};
use crate::tidal::Quality;
use crate::tidal::models::PlaybackInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Flac,
    /// MQA in a FLAC container (TIDAL's retired "Master" tier).
    Mqa,
    Aac,
    /// Dolby Atmos.
    Eac3,
    Ac4,
    Unknown,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Self::Flac => "FLAC",
            Self::Mqa => "MQA",
            Self::Aac => "AAC",
            Self::Eac3 => "E-AC-3",
            Self::Ac4 => "AC-4",
            Self::Unknown => tr("Unknown"),
        }
    }

    pub fn is_lossless(self) -> bool {
        matches!(self, Self::Flac | Self::Mqa)
    }

    /// From a manifest's `codecs` value (`flac`, `mp4a.40.2`, `eac3`...),
    /// falling back to TIDAL's quality tier.
    fn detect(codecs: Option<&str>, audio_quality: Option<&str>) -> Self {
        let codecs = codecs.map(str::to_ascii_lowercase).unwrap_or_default();
        match codecs.as_str() {
            c if c.starts_with("flac") => Self::Flac,
            c if c.starts_with("mqa") => Self::Mqa,
            c if c.starts_with("mp4a") || c.starts_with("aac") => Self::Aac,
            c if c.starts_with("eac3") || c.starts_with("ec-3") => Self::Eac3,
            c if c.starts_with("ac4") || c.starts_with("ac-4") => Self::Ac4,
            _ => match audio_quality {
                Some("HI_RES_LOSSLESS" | "LOSSLESS") => Self::Flac,
                Some("HI_RES") => Self::Mqa,
                Some("HIGH" | "LOW") => Self::Aac,
                _ => Self::Unknown,
            },
        }
    }
}

/// The format TIDAL delivered for the current track.
#[derive(Clone, Debug, PartialEq)]
pub struct FormatDetails {
    pub codec: Codec,
    pub bit_depth: Option<u32>,
    /// Hz.
    pub sample_rate: Option<u32>,
    /// kbit/s, for lossy codecs.
    pub bitrate: Option<u32>,
    /// The tier TIDAL answered with (may be below the one requested when
    /// a track is not available in it).
    pub quality: Option<Quality>,
    /// "STEREO", "DOLBY_ATMOS"...
    pub audio_mode: Option<String>,
    /// The ReplayGain adjustment being applied in dB; `None` when
    /// normalisation is off (bit-perfect output).
    pub replay_gain_db: Option<f64>,
}

impl FormatDetails {
    /// `codecs` is the manifest's codec string, when it has one.
    pub fn new(info: &PlaybackInfo, codecs: Option<&str>) -> Self {
        let tier = info.audio_quality.as_deref();
        let codec = Codec::detect(codecs, tier);
        let quality = tier.and_then(parse_quality);
        let (mut bit_depth, mut sample_rate) = (info.bit_depth, info.sample_rate);
        if codec.is_lossless() && quality == Some(Quality::Lossless) {
            // CD quality is always 16/44.1; older answers leave it out.
            bit_depth = bit_depth.or(Some(16));
            sample_rate = sample_rate.or(Some(44_100));
        }
        let bitrate = match (codec, quality) {
            (Codec::Aac, Some(Quality::High)) => Some(320),
            (Codec::Aac, Some(Quality::Low)) => Some(96),
            _ => None,
        };
        Self {
            codec,
            bit_depth: bit_depth.filter(|_| codec.is_lossless()),
            sample_rate,
            bitrate,
            quality,
            audio_mode: info.audio_mode.clone(),
            replay_gain_db: None,
        }
    }

    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn is_lossless(&self) -> bool {
        self.codec.is_lossless()
    }

    /// More than CD quality.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn is_hires(&self) -> bool {
        self.is_lossless()
            && (self.bit_depth.is_some_and(|b| b > 16)
                || self.sample_rate.is_some_and(|r| r > 48_000))
    }

    /// "24-bit/96 kHz FLAC", "16-bit/44.1 kHz FLAC", "AAC 320 kbit/s".
    pub fn describe(&self) -> String {
        match self.codec {
            Codec::Flac | Codec::Mqa => match (self.bit_depth, self.sample_rate) {
                (Some(bits), Some(rate)) => tr("{bits}-bit/{rate} kHz {codec}")
                    .replace("{bits}", &bits.to_string())
                    .replace("{rate}", &khz(rate))
                    .replace("{codec}", self.codec.name()),
                _ if self.quality == Some(Quality::HiResLossless) => {
                    format!("Hi-Res {}", self.codec.name())
                }
                _ => self.codec.name().to_owned(),
            },
            Codec::Aac => match self.bitrate {
                Some(kbps) => format!("AAC {kbps} kbit/s"),
                None => "AAC".to_owned(),
            },
            Codec::Eac3 | Codec::Ac4 => format!("Dolby Atmos ({})", self.codec.name()),
            Codec::Unknown => tr("Unknown format").to_owned(),
        }
    }

    /// Compact form for badges: "FLAC 24/96", "AAC 320".
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn short(&self) -> String {
        match (self.codec, self.bit_depth, self.sample_rate, self.bitrate) {
            (codec, Some(bits), Some(rate), _) if codec.is_lossless() => {
                format!("{} {bits}/{}", codec.name(), khz(rate))
            }
            (codec, _, _, Some(kbps)) => format!("{} {kbps}", codec.name()),
            (codec, ..) => codec.name().to_owned(),
        }
    }
}

/// 44100 → "44.1", 96000 → "96", 22050 → "22.05".
pub fn khz(rate: u32) -> String {
    let (whole, frac) = (rate / 1000, rate % 1000);
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{}", format!("{frac:03}").trim_end_matches('0'))
    }
}

pub fn parse_quality(name: &str) -> Option<Quality> {
    match name {
        "HI_RES_LOSSLESS" => Some(Quality::HiResLossless),
        "LOSSLESS" | "HI_RES" => Some(Quality::Lossless),
        "HIGH" => Some(Quality::High),
        "LOW" => Some(Quality::Low),
        _ => None,
    }
}

fn rank(quality: Quality) -> u8 {
    match quality {
        Quality::Low => 0,
        Quality::High => 1,
        Quality::Lossless => 2,
        Quality::HiResLossless => 3,
    }
}

/// The tier to ask TIDAL for: the preference, capped by what the login
/// allows.
pub fn requested_quality(preference: QualityPreference, max: Quality) -> Quality {
    let wanted = match preference {
        QualityPreference::Best => Quality::HiResLossless,
        QualityPreference::Lossless => Quality::Lossless,
        QualityPreference::High => Quality::High,
        QualityPreference::Low => Quality::Low,
    };
    if rank(wanted) <= rank(max) {
        wanted
    } else {
        max
    }
}

/// ReplayGain values TIDAL sends with the stream: gains in dB, peaks as
/// linear amplitude (1.0 = full scale).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGain {
    pub track_gain: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_gain: Option<f64>,
    pub album_peak: Option<f64>,
}

impl ReplayGain {
    pub fn new(info: &PlaybackInfo) -> Self {
        Self {
            track_gain: info.track_replay_gain,
            track_peak: info.track_peak_amplitude,
            album_gain: info.album_replay_gain,
            album_peak: info.album_peak_amplitude,
        }
    }

    /// The linear factor to scale samples by, or `None` for no filter at
    /// all (normalisation off). Album mode falls back to the track values
    /// and vice versa; without any value the factor is 1. The factor never
    /// lets the peak clip.
    pub fn factor(&self, mode: Normalization) -> Option<f64> {
        let track = self.track_gain.map(|g| (g, self.track_peak));
        let album = self.album_gain.map(|g| (g, self.album_peak));
        let chosen = match mode {
            Normalization::Off => return None,
            Normalization::Track => track.or(album),
            Normalization::Album => album.or(track),
        };
        let Some((gain, peak)) = chosen.filter(|(g, _)| g.is_finite()) else {
            return Some(1.0);
        };
        let mut factor = 10f64.powf(gain / 20.0);
        if let Some(peak) = peak.filter(|p| p.is_finite() && *p > 0.0) {
            factor = factor.min(1.0 / peak);
        }
        Some(factor.clamp(0.0, 10.0))
    }
}

/// A linear factor in dB.
pub fn to_db(factor: f64) -> f64 {
    20.0 * factor.max(1e-9).log10()
}

/// Perceptual volume: the slider position (0–1) cubed, the amplitude
/// factor. PulseAudio/PipeWire mixers use the same curve, so the system
/// mixer shows the same percentage as the slider.
pub fn volume_curve(linear: f64) -> f64 {
    linear.clamp(0.0, 1.0).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(quality: &str, bits: Option<u32>, rate: Option<u32>) -> PlaybackInfo {
        PlaybackInfo {
            audio_quality: Some(quality.to_owned()),
            bit_depth: bits,
            sample_rate: rate,
            ..PlaybackInfo::default()
        }
    }

    #[test]
    fn describes_formats() {
        if crate::i18n::ui_language().is_some() {
            // Checks the English texts.
            return;
        }
        let hires = FormatDetails::new(
            &info("HI_RES_LOSSLESS", Some(24), Some(96_000)),
            Some("flac"),
        );
        assert_eq!(hires.describe(), "24-bit/96 kHz FLAC");
        assert_eq!(hires.short(), "FLAC 24/96");
        assert!(hires.is_hires());

        let cd = FormatDetails::new(&info("LOSSLESS", None, None), None);
        assert_eq!(cd.describe(), "16-bit/44.1 kHz FLAC");
        assert!(!cd.is_hires());

        let odd = FormatDetails::new(
            &info("HI_RES_LOSSLESS", Some(24), Some(88_200)),
            Some("flac"),
        );
        assert_eq!(odd.describe(), "24-bit/88.2 kHz FLAC");

        let aac = FormatDetails::new(&info("HIGH", Some(16), Some(44_100)), Some("mp4a.40.2"));
        assert_eq!(aac.describe(), "AAC 320 kbit/s");
        assert_eq!(aac.bit_depth, None);
        assert_eq!(aac.short(), "AAC 320");

        let low = FormatDetails::new(&info("LOW", None, None), None);
        assert_eq!(low.describe(), "AAC 96 kbit/s");
    }

    #[test]
    fn khz_formatting() {
        assert_eq!(khz(44_100), "44.1");
        assert_eq!(khz(48_000), "48");
        assert_eq!(khz(176_400), "176.4");
        assert_eq!(khz(22_050), "22.05");
    }

    #[test]
    fn quality_is_capped() {
        use QualityPreference::*;
        assert_eq!(
            requested_quality(Best, Quality::Lossless),
            Quality::Lossless
        );
        assert_eq!(
            requested_quality(Best, Quality::HiResLossless),
            Quality::HiResLossless
        );
        assert_eq!(
            requested_quality(High, Quality::HiResLossless),
            Quality::High
        );
        assert_eq!(requested_quality(Lossless, Quality::High), Quality::High);
    }

    #[test]
    fn replay_gain() {
        let gain = ReplayGain {
            track_gain: Some(-6.0),
            track_peak: Some(0.9),
            album_gain: Some(4.0),
            album_peak: Some(0.8),
        };
        assert_eq!(gain.factor(Normalization::Off), None);
        let track = gain.factor(Normalization::Track).unwrap();
        assert!((to_db(track) + 6.0).abs() < 1e-9);
        // +4 dB would push a 0.8 peak past full scale: limited to 1/0.8.
        let album = gain.factor(Normalization::Album).unwrap();
        assert!((album - 1.25).abs() < 1e-9);

        let only_track = ReplayGain {
            track_gain: Some(-3.0),
            ..ReplayGain::default()
        };
        assert!((to_db(only_track.factor(Normalization::Album).unwrap()) + 3.0).abs() < 1e-9);
        assert_eq!(
            ReplayGain::default().factor(Normalization::Track),
            Some(1.0)
        );
    }

    #[test]
    fn volume_curve_is_cubic() {
        assert_eq!(volume_curve(1.0), 1.0);
        assert_eq!(volume_curve(0.0), 0.0);
        assert!((volume_curve(0.5) - 0.125).abs() < 1e-12);
        assert_eq!(volume_curve(2.0), 1.0);
    }
}
