//! Страница плеера и иконки вшиты прямо в exe — его можно положить куда угодно
//! одним файлом.

#[derive(Clone, Copy)]
pub struct Asset {
    pub mime: &'static str,
    pub cache: &'static str,
    pub body: &'static [u8],
}

const DAY: &str = "public, max-age=86400";

const PAGE: Asset = Asset {
    mime: "text/html; charset=utf-8",
    cache: "no-cache",
    body: include_bytes!("../public/listen.html"),
};

const fn png(body: &'static [u8]) -> Asset {
    Asset {
        mime: "image/png",
        cache: DAY,
        body,
    }
}

pub fn find(path: &str) -> Option<Asset> {
    Some(match path {
        // Единственная страница — плеер слушателя.
        "/" | "/listen" | "/listen.html" => PAGE,
        "/favicon.png" | "/favicon.ico" => png(include_bytes!("../public/favicon.png")),
        "/icon-180.png" => png(include_bytes!("../public/icon-180.png")),
        "/icon-192.png" => png(include_bytes!("../public/icon-192.png")),
        "/icon-512.png" => png(include_bytes!("../public/icon-512.png")),
        "/manifest.webmanifest" => Asset {
            mime: "application/manifest+json",
            cache: DAY,
            body: include_bytes!("../public/manifest.webmanifest"),
        },
        _ => return None,
    })
}
