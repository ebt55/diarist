//! Asset-protocol URLs for local files (what `convertFileSrc` builds in the
//! page), so playback streams the file with Range requests.

use std::path::Path;

/// The asset-protocol URL for a local file, as `convertFileSrc` builds it.
pub fn asset_url(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut encoded = String::with_capacity(raw.len() * 3);
    for b in raw.bytes() {
        // encodeURIComponent leaves exactly these unescaped.
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    if cfg!(any(windows, target_os = "android")) {
        format!("http://asset.localhost/{encoded}")
    } else {
        format!("asset://localhost/{encoded}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_url_matches_encode_uri_component() {
        let url = asset_url(Path::new("C:\\Users\\me\\a b(1).wav"));
        let tail = url.rsplit('/').next().unwrap();
        assert_eq!(tail, "C%3A%5CUsers%5Cme%5Ca%20b(1).wav");
    }
}
