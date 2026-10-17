//! Application identity and the TIDAL OAuth clients Swell signs in with.

pub const APP_ID: &str = "io.github.oxakromax.SwellPlayer";
pub const APP_NAME: &str = "Swell";

/// An OAuth client registered by TIDAL for one of its own apps.
///
/// TIDAL does not let third parties register clients for its (unofficial)
/// v1 API, so every open-source client reuses the public ids of TIDAL's apps.
/// Which one is used decides the best audio quality the session can stream.
pub struct OAuthClient {
    pub id: &'static str,
    pub secret: &'static str,
}

/// TIDAL's Android app. Used with the PKCE browser login; the only client
/// granted `HI_RES_LOSSLESS` (FLAC up to 24-bit/192 kHz, served as MPEG-DASH).
pub const PKCE_CLIENT: OAuthClient = OAuthClient {
    id: "6BDSRdpK9hqEBTgU",
    secret: "xeuPmY7nbpZ9IIbLAcQ93shka1VNheUAqN6IcszjTG8=",
};

/// TIDAL's TV/device app. Used with the device-code login; capped at
/// `LOSSLESS` (FLAC 16-bit/44.1 kHz).
pub const DEVICE_CLIENT: OAuthClient = OAuthClient {
    id: "fX2JxdmntZWK0ixT",
    secret: "1Nn9AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg=",
};

/// Where TIDAL's login page sends the browser after a PKCE sign-in. Nothing is
/// served there for us: the user copies that address back into Swell.
pub const PKCE_REDIRECT_URI: &str = "https://tidal.com/android/login/auth";
