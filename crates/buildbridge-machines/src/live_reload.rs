//! Development server addresses carried by debug builds, never by release recipes.

/// Normalize a credential-free HTTP(S) development URL without accepting browser URL
/// repairs (backslashes, whitespace or ambiguous numeric hosts). Paths are preserved.
pub fn normalize_live_reload_url(value: &str) -> Result<String, String> {
    parse_live_reload_url(value).map(|url| url.normalized)
}

/// An iPhone reaches the development server over its network. Unlike Android, it has no
/// ADB reverse route from phone-local addresses back to the computer running buildbridge.
pub fn normalize_apple_live_reload_url(value: &str) -> Result<String, String> {
    let url = parse_live_reload_url(value).map_err(|_| {
        "Enter an HTTP or HTTPS development server URL your iPhone can reach, without credentials, a query or a fragment. Use this computer's LAN address rather than localhost.".to_string()
    })?;
    let authority = url
        .normalized
        .split_once("://")
        .expect("validated URL scheme")
        .1
        .split('/')
        .next()
        .expect("validated URL host");
    let host = authority.split(':').next().unwrap_or_default();
    if url.reverse_port.is_some() || host.ends_with(".localhost") {
        return Err("Use this computer's LAN address or a server hostname your iPhone can reach. Localhost on the iPhone refers to the phone itself.".to_string());
    }
    Ok(url.normalized)
}

/// A loopback address in an Android web view names the phone. ADB reverse makes that port
/// reach this computer instead, carrying HTTP and the dev server's WebSocket on one port.
pub fn live_reload_reverse_port(value: &str) -> Result<Option<u16>, String> {
    parse_live_reload_url(value).map(|url| url.reverse_port)
}

struct LiveReloadUrl {
    normalized: String,
    reverse_port: Option<u16>,
}

fn parse_live_reload_url(value: &str) -> Result<LiveReloadUrl, String> {
    let invalid = || {
        "Enter an HTTP or HTTPS development server URL without credentials, a query or a fragment (for example http://localhost:5173).".to_string()
    };
    let value = value.trim();
    if value.len() > 2048
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || value.contains(['\\', '?', '#'])
    {
        return Err(invalid());
    }
    let (scheme, rest) = value.split_once("://").ok_or_else(invalid)?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err(invalid());
    }
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, ""), |(host, path)| (host, path));
    if authority.is_empty() || authority.contains('@') {
        return Err(invalid());
    }
    let (host, explicit_port) = if authority.starts_with('[') {
        let (host, suffix) = authority.split_once(']').ok_or_else(invalid)?;
        let address = host
            .strip_prefix('[')
            .ok_or_else(invalid)?
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| invalid())?;
        if address.is_unspecified()
            || address.is_loopback()
            || address
                .to_ipv4_mapped()
                .is_some_and(|address| address.is_loopback() || address.is_unspecified())
        {
            return Err(
                "Use localhost for a server on this computer, or a reachable network address."
                    .into(),
            );
        }
        let port = if suffix.is_empty() {
            None
        } else {
            Some(suffix.strip_prefix(':').ok_or_else(invalid)?)
        };
        (format!("[{address}]"), port)
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if host.is_empty()
            || host.len() > 253
            || !host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return Err(invalid());
        }
        // Browsers accept shortened/octal/hex IPv4 spellings; refusing them keeps the host
        // interpreted here equal to the host Capacitor's web view will actually connect to.
        if host.rsplit('.').next().is_some_and(|label| {
            label.bytes().all(|byte| byte.is_ascii_digit())
                || label.to_ascii_lowercase().starts_with("0x")
        }) {
            let address = host.parse::<std::net::Ipv4Addr>().map_err(|_| invalid())?;
            if address.is_unspecified()
                || (address.is_loopback() && address != std::net::Ipv4Addr::LOCALHOST)
                || host != address.to_string()
            {
                return Err(invalid());
            }
        }
        (host.to_ascii_lowercase(), port)
    };
    let port = explicit_port
        .map(|port| {
            if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid());
            }
            port.parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .ok_or_else(invalid)
        })
        .transpose()?
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let path_bytes = path.as_bytes();
    for (index, byte) in path_bytes.iter().enumerate() {
        if !(byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/%".contains(byte))
            || (*byte == b'%'
                && !path_bytes
                    .get(index + 1..index + 3)
                    .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit)))
        {
            return Err(invalid());
        }
    }
    let port_suffix = explicit_port
        .map(|_| format!(":{port}"))
        .unwrap_or_default();
    let normalized = format!("{scheme}://{host}{port_suffix}/{path}");
    Ok(LiveReloadUrl {
        normalized,
        reverse_port: matches!(host.as_str(), "localhost" | "127.0.0.1").then_some(port),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_urls_are_normalized_and_only_loopback_uses_adb_reverse() {
        assert_eq!(
            normalize_live_reload_url(" HTTP://LOCALHOST:05173 ").unwrap(),
            "http://localhost:5173/"
        );
        assert_eq!(
            normalize_live_reload_url("https://dev.example/app/%20assets/").unwrap(),
            "https://dev.example/app/%20assets/"
        );
        for (url, expected) in [
            ("http://localhost:5173", Some(5173)),
            ("https://127.0.0.1", Some(443)),
            ("http://192.168.1.2:5173", None),
            ("https://dev.example", None),
        ] {
            assert_eq!(live_reload_reverse_port(url).unwrap(), expected);
        }
    }

    #[test]
    fn ambiguous_or_sensitive_addresses_are_rejected() {
        for url in [
            "",
            "ftp://localhost:5173",
            "http://user:pass@localhost",
            "http://localhost/?token=secret",
            "http://localhost/#fragment",
            "http://localhost:0",
            "http://localhost:65536",
            "http://localhost:abc",
            "http://127.1",
            "http://0177.0.0.1",
            "http://0x7f000001",
            "http://0X7F000001",
            "http://127.0.0.2:5173",
            "http://[::1",
            "http://host\\path",
            "http://host/a\nb",
            "http://host/%oops",
            "http://host/a b",
            "http://-host",
            "http://host..test",
            "http://0.0.0.0:5173",
            "http://[::]:5173",
            "http://[::1]:5173",
            "http://[::ffff:127.0.0.1]:5173",
            "http://[[2001:db8::1]:5173",
        ] {
            assert!(normalize_live_reload_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn iphone_servers_require_a_phone_reachable_address() {
        for value in [
            "http://192.168.1.10:5173",
            "https://dev.example/app/",
            "http://workstation.local:8100",
            "https://[2001:db8::1]:5173/",
        ] {
            assert!(normalize_apple_live_reload_url(value).is_ok(), "{value}");
        }
        for value in [
            "http://localhost:5173",
            "http://LOCALHOST:5173",
            "http://localhost.:5173",
            "http://app.localhost:5173",
            "http://127.0.0.1:5173",
            "http://127.0.0.2:5173",
            "http://127.1:5173",
            "http://0x7f000001:5173",
            "http://[::1]:5173",
            "http://[::ffff:127.0.0.1]:5173",
            "http://0.0.0.0:5173",
            "http://[::]:5173",
        ] {
            let error = normalize_apple_live_reload_url(value).unwrap_err();
            assert!(error.contains("iPhone"), "{error}");
        }
    }
}
