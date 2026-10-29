//! API 0.5 `http`: HTTPS requests to hosts the user allows
//! (`allow-network`). Everything a request may do is checked here; the
//! editor sends it with the system's `curl` and checks the address the
//! host resolves to (see `helix-term/src/plugins/http.rs`).

use std::net::IpAddr;

/// Longest URL.
pub const MAX_URL_LEN: usize = 8192;
/// Most request headers.
pub const MAX_HEADERS: usize = 64;
/// Largest request body.
pub const MAX_BODY: usize = 4 << 20;
/// Largest response body.
pub const MAX_RESPONSE: usize = 16 << 20;
/// Default and longest time for one request, in seconds.
pub const DEFAULT_TIMEOUT: u32 = 30;
pub const MAX_TIMEOUT: u32 = 120;
/// Most requests in flight per plugin.
pub const MAX_IN_FLIGHT: usize = 8;

const METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];

/// A checked request, ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    /// Lowercase; IPv6 without brackets.
    pub host: String,
    pub port: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// Seconds.
    pub timeout: u32,
}

/// Whether `pattern` from `allow-network` is valid: a host name, an IP
/// address, or `*.` and a domain (its subdomains, not itself).
pub fn valid_pattern(pattern: &str) -> bool {
    let name = pattern.strip_prefix("*.").unwrap_or(pattern);
    pattern.parse::<IpAddr>().is_ok() || valid_host_name(name) && name.contains('.')
}

fn valid_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// Whether `host` is one of `allowed`.
pub fn allowed(host: &str, allowed: &[String]) -> bool {
    allowed
        .iter()
        .any(|pattern| match pattern.strip_prefix("*.") {
            Some(domain) => host
                .strip_suffix(domain)
                .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
            None => host == pattern,
        })
}

/// The host and port of an `https://` URL.
fn host_port(url: &str) -> Result<(String, u16), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or("only https:// URLs are allowed")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return Err("URLs with a user name or password aren't allowed".into());
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']').ok_or("invalid IPv6 address")?;
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| "invalid IPv6 address")?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        None => 443,
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| format!("invalid port '{port}'"))?,
    };
    let host = host.to_ascii_lowercase();
    if host.parse::<IpAddr>().is_err() && !valid_host_name(&host) {
        return Err(format!("invalid host '{host}'"));
    }
    Ok((host, port))
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

/// Headers the editor sets (or that would change how the request is sent).
const RESERVED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "proxy-authorization",
    "te",
    "trailer",
];

/// Check a plugin's request against what the user allows.
pub fn check(
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<String>,
    timeout: Option<u32>,
    allowed_hosts: &[String],
) -> Result<HttpRequest, String> {
    if !METHODS.contains(&method) {
        return Err(format!(
            "unsupported method '{method}': use one of {}",
            METHODS.join(", ")
        ));
    }
    if url.len() > MAX_URL_LEN {
        return Err(format!("the URL is longer than {MAX_URL_LEN} bytes"));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the URL contains spaces or control characters".into());
    }
    let (host, port) = host_port(url)?;
    if !allowed(&host, allowed_hosts) {
        return Err(format!(
            "'{host}' isn't in allow-network for this plugin in plugins.toml"
        ));
    }
    if headers.len() > MAX_HEADERS {
        return Err(format!("more than {MAX_HEADERS} headers"));
    }
    for (name, value) in &headers {
        if !valid_header_name(name) {
            return Err(format!("invalid header name '{name}'"));
        }
        if RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(format!("the '{name}' header is set by the editor"));
        }
        if value.len() > 16384 || value.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
            return Err(format!("invalid value for header '{name}'"));
        }
    }
    if body.as_ref().is_some_and(|body| body.len() > MAX_BODY) {
        return Err(format!("the body is larger than {} MiB", MAX_BODY >> 20));
    }
    let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
    if timeout == 0 || timeout > MAX_TIMEOUT {
        return Err(format!("a timeout of 1 to {MAX_TIMEOUT} seconds"));
    }
    Ok(HttpRequest {
        method: method.to_owned(),
        url: url.to_owned(),
        host,
        port,
        headers,
        body,
        timeout,
    })
}

/// Whether a plugin may connect to `ip`: only public unicast addresses, never
/// loopback, private, link-local, shared (CGNAT), multicast and the like.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_documentation()
                || a == 0
                // Shared address space (RFC 6598).
                || a == 100 && (64..128).contains(&b)
                // IETF protocol assignments, benchmarking, reserved.
                || a == 192 && b == 0 && ip.octets()[2] == 0
                || a == 198 && (18..20).contains(&b)
                || a >= 240)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = ip.segments()[0];
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                // Unique local (fc00::/7) and link-local (fe80::/10).
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                // Documentation (2001:db8::/32).
                || first == 0x2001 && ip.segments()[1] == 0xdb8
                // NAT64 and 6to4 can reach IPv4 addresses.
                || first == 0x64
                || first == 0x2002)
        }
    }
}

/// The response as plugins get it: a JSON object with `status`, `headers`
/// (pairs) and `body` (text; bytes that aren't UTF-8 are replaced).
pub fn response_json(status: u16, headers: &[(String, String)], body: &[u8]) -> String {
    serde_json::json!({
        "status": status,
        "headers": headers,
        "body": String::from_utf8_lossy(body),
    })
    .to_string()
}

#[cfg(test)]
mod test {
    use super::*;

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn patterns() {
        for good in ["api.github.com", "*.example.org", "1.2.3.4", "::1"] {
            assert!(valid_pattern(good), "{good}");
        }
        for bad in [
            "",
            "*",
            "*.com.",
            "localhost",
            "API.github.com",
            "a/b.com",
            "*.*.com",
            "x..y",
        ] {
            assert!(!valid_pattern(bad), "{bad}");
        }
        let list = hosts(&["api.github.com", "*.example.org"]);
        assert!(allowed("api.github.com", &list));
        assert!(allowed("a.b.example.org", &list));
        assert!(!allowed("example.org", &list));
        assert!(!allowed("evilexample.org", &list));
        assert!(!allowed("github.com", &list));
        assert!(!allowed("api.github.com.evil.net", &list));
    }

    #[test]
    fn requests_are_checked() {
        let list = hosts(&["api.example.com", "*.example.org", "2001:4860::8888"]);
        let ok = |url: &str| check("GET", url, vec![], None, None, &list);
        let request = ok("https://API.example.com/v1?q=1").unwrap();
        assert_eq!(
            (request.host.as_str(), request.port),
            ("api.example.com", 443)
        );
        assert_eq!(ok("https://x.example.org:8443/").unwrap().port, 8443);
        assert_eq!(
            ok("https://[2001:4860::8888]/").unwrap().host,
            "2001:4860::8888"
        );
        for (url, why) in [
            ("http://api.example.com/", "only https"),
            ("https://example.com/", "allow-network"),
            ("https://user:pw@api.example.com/", "user name"),
            ("https://api.example.com:0/", "invalid port"),
            ("https://api.example.com:99999/", "invalid port"),
            ("https://api.example.com/a b", "spaces"),
            ("https://api.example.com/\r\nX: y", "control"),
            ("https://api.example.com.evil.net/", "allow-network"),
            ("https://[::1/", "IPv6"),
            ("file:///etc/passwd", "only https"),
        ] {
            let err = ok(url).unwrap_err();
            assert!(err.contains(why), "{url}: {err}");
        }
        let with = |method: &str, headers: Vec<(&str, &str)>, timeout: Option<u32>| {
            check(
                method,
                "https://api.example.com/",
                headers
                    .into_iter()
                    .map(|(n, v)| (n.to_string(), v.to_string()))
                    .collect(),
                None,
                timeout,
                &list,
            )
        };
        assert!(with("POST", vec![("Authorization", "Bearer x")], Some(5)).is_ok());
        assert!(with("CONNECT", vec![], None)
            .unwrap_err()
            .contains("unsupported"));
        assert!(with("GET", vec![("Host", "evil")], None)
            .unwrap_err()
            .contains("set by the editor"));
        assert!(with("GET", vec![("X", "a\r\nB: c")], None)
            .unwrap_err()
            .contains("invalid value"));
        // `curl -H @file` reads a file: names can't start with "@".
        assert!(with("GET", vec![("@/etc/passwd", "")], None)
            .unwrap_err()
            .contains("invalid header name"));
        assert!(with("GET", vec![], Some(0)).is_err());
        assert!(with("GET", vec![], Some(MAX_TIMEOUT + 1)).is_err());
    }

    #[test]
    fn only_public_addresses() {
        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "198.18.0.1",
            "240.0.0.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
    }
}
