//! Pictures inside an email, fetched only when the person asks to see them. They are loaded here, not by the page, so the
//! sender never sees the app itself, and only real pictures from public web servers are accepted: not addresses on this
//! computer or network, not scripts, not vector files that can carry code.

use std::net::IpAddr;
use std::time::Duration;

use base64::Engine;

const MAX_BYTES: usize = 4_000_000;

fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80)
        }
    }
}

fn picture_type(content_type: &str) -> Option<&'static str> {
    match content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "image/bmp" => Some("image/bmp"),
        "image/avif" => Some("image/avif"),
        _ => None,
    }
}

/// The picture at `address` as a `data:` address, or nothing if it can't be loaded safely.
pub async fn fetch(address: &str) -> Option<String> {
    let mut url = url::Url::parse(address).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let _ = url.set_scheme("https");
    let host = url.host_str()?.to_string();
    let port = url.port_or_known_default()?;
    let resolved: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await.ok()?.collect();
    // Every address the name resolves to must be public, and the connection is pinned to one of them.
    if resolved.is_empty() || !resolved.iter().all(|a| public(a.ip())) {
        return None;
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .resolve(&host, resolved[0])
        .user_agent("Mozilla/5.0")
        .build()
        .ok()?;
    let mut response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let kind = picture_type(response.headers().get(reqwest::header::CONTENT_TYPE)?.to_str().ok()?)?;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    (!bytes.is_empty()).then(|| format!("data:{kind};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_are_allowed() {
        assert!(public("93.184.216.34".parse().unwrap()));
        for private in ["127.0.0.1", "10.0.0.5", "192.168.1.1", "169.254.169.254", "0.0.0.0", "100.64.0.1", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1"] {
            assert!(!public(private.parse().unwrap()), "{private}");
        }
    }

    #[tokio::test]
    async fn local_and_odd_addresses_are_refused() {
        for address in ["http://127.0.0.1/a.png", "https://localhost/a.png", "ftp://example.com/a.png", "https://user:pw@example.com/a.png", "data:image/png;base64,AA==", "not a url"] {
            assert!(fetch(address).await.is_none(), "{address}");
        }
    }
}
