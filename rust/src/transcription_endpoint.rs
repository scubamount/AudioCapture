//! Shared URL policy for transcription configuration and WinHTTP.
use std::net::IpAddr;

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub secure: bool,
}

/// Parse an API base URL and preserve any reverse-proxy path prefix.
pub fn endpoint(base: &str, allow_http: bool) -> Result<Endpoint, String> {
    if base.chars().any(|c| c.is_control() || c.is_whitespace()) || base.contains(['?', '#', '\\'])
    {
        return Err("Endpoint URL contains unsupported characters".into());
    }
    let (secure, rest, default_port) = if let Some(rest) = base.strip_prefix("https://") {
        (true, rest, 443)
    } else if let Some(rest) = base.strip_prefix("http://") {
        (false, rest, 80)
    } else {
        return Err("Endpoint must use HTTP or HTTPS".into());
    };
    let (authority, prefix) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, String::new()),
    };
    if authority.is_empty() || authority.contains(['@', '%']) {
        return Err("Endpoint has an invalid host or embedded credentials".into());
    }
    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let (host, suffix) = ipv6.split_once(']').ok_or("Invalid IPv6 endpoint")?;
        let _: std::net::Ipv6Addr = host.parse().map_err(|_| "Invalid IPv6 endpoint")?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            parse_port(suffix.strip_prefix(':').ok_or("Invalid endpoint port")?)?
        };
        (host.to_string(), port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, parse_port(port)?),
            None => (authority, default_port),
        };
        if host.is_empty()
            || !host.is_ascii()
            || !host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        {
            return Err("Invalid endpoint host".into());
        }
        (host.to_string(), port)
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if !secure && !loopback && !allow_http {
        return Err("Non-loopback HTTP requires --allow-http".into());
    }
    if prefix.split('/').any(|part| part == "." || part == "..") {
        return Err("Endpoint path cannot contain dot segments".into());
    }
    let path = format!("{}/audio/transcriptions", prefix.trim_end_matches('/'));
    Ok(Endpoint {
        host,
        port,
        path,
        secure,
    })
}

/// Identify the effective destination, including its service path.
pub fn canonical_key(endpoint: &Endpoint) -> String {
    let scheme = if endpoint.secure { "https" } else { "http" };
    let host = endpoint.host.to_ascii_lowercase();
    let authority = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    format!("{scheme}://{authority}:{}{}", endpoint.port, endpoint.path)
}

fn parse_port(port: &str) -> Result<u16, String> {
    let port: u16 = port.parse().map_err(|_| "Invalid endpoint port")?;
    if port == 0 {
        return Err("Invalid endpoint port".into());
    }
    Ok(port)
}
