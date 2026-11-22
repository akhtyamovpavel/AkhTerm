//! Embedded lightweight Dark/Light Zinc Web/PWA client served at `/` and `/manifest.webmanifest`.

pub const PWA_MANIFEST_JSON: &str = concat!(
    "{",
    "\"name\":\"AkhTerm Remote\",",
    "\"short_name\":\"AkhTerm\",",
    "\"start_url\":\"/\",",
    "\"display\":\"standalone\",",
    "\"background_color\":\"#09090b\",",
    "\"theme_color\":\"#09090b\"",
    "}"
);

pub const PWA_INDEX_HTML: &str = include_str!("pwa.html");
