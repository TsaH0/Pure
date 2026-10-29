//! HTTPS requests for plugins (API 0.5 `http`), sent with the system's
//! `curl`. `helix_plugin::http::check` already vetted the request against
//! the user's `allow-network`; here the host's addresses are checked too
//! (only public ones) and curl is pinned to the one checked, so DNS can't
//! point a request at a local service.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use helix_event::runtime_local;
use helix_plugin::http::{self, HttpRequest};
use helix_plugin::v05::Callback2;
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

runtime_local! {
    /// Requests in flight by `(plugin, request id)`; aborting one kills curl.
    static REQUESTS: Mutex<BTreeMap<(String, u32), tokio::task::AbortHandle>> =
        Mutex::new(BTreeMap::new());
}

/// Send `request` for `plugin`; the response arrives as an `HttpReply`.
pub fn fetch(plugin: String, id: u32, request: HttpRequest) {
    let key = (plugin.clone(), id);
    // Held until the task is in the map, so it can't remove itself first.
    let mut requests = REQUESTS.lock();
    let task = tokio::spawn(async move {
        let result = send(&request).await;
        REQUESTS.lock().remove(&(plugin.clone(), id));
        super::deliver2_later(plugin, Callback2::HttpReply { id, result });
    });
    requests.insert(key, task.abort_handle());
}

pub fn cancel(plugin: &str, id: u32) {
    if let Some(task) = REQUESTS.lock().remove(&(plugin.to_string(), id)) {
        task.abort();
    }
}

/// Stop `plugin`'s requests, or everyone's.
pub fn stop_plugin(plugin: Option<&str>) {
    REQUESTS.lock().retain(|(owner, _), task| {
        let keep = plugin.is_some_and(|plugin| plugin != owner);
        if !keep {
            task.abort();
        }
        keep
    });
}

/// Whether curl will go through a proxy: then the proxy connects, and the
/// address check can't apply.
fn proxied() -> bool {
    ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// Proxy and certificate settings curl may read from the environment.
const PASSED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "SYSTEMROOT",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "no_proxy",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
];

async fn send(request: &HttpRequest) -> Result<String, String> {
    let mut command = tokio::process::Command::new("curl");
    // `-q` first: no ~/.curlrc.
    command.args([
        "-q",
        "--silent",
        "--show-error",
        "--globoff",
        "--proto",
        "=https",
        "--max-redirs",
        "0",
        "--include",
        "--suppress-connect-headers",
        "--max-time",
        &request.timeout.to_string(),
        "--max-filesize",
        &http::MAX_RESPONSE.to_string(),
        "--request",
        &request.method,
    ]);
    if request.method == "HEAD" {
        command.arg("--head");
    }
    // Pin curl to a checked address: it connects directly when there's no
    // proxy, or when `no_proxy` names the host. Through a proxy that does
    // the resolving, a name the editor can't resolve is left to it.
    if let Some(address) = pinned_address(request).await? {
        let bracket = |host: String| {
            if host.contains(':') {
                format!("[{host}]")
            } else {
                host
            }
        };
        command.arg("--resolve").arg(format!(
            "{}:{}:{}",
            bracket(request.host.clone()),
            request.port,
            bracket(address.to_string())
        ));
    }
    for (name, value) in &request.headers {
        command.arg("--header").arg(format!("{name}: {value}"));
    }
    if request.body.is_some() {
        command.args(["--data-binary", "@-"]);
    }
    command
        .arg("--url")
        .arg(&request.url)
        .env_clear()
        .envs(
            PASSED_ENV
                .iter()
                .filter_map(|name| Some((name, std::env::var_os(name)?))),
        )
        .stdin(if request.body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => "HTTPS requests need curl installed".to_string(),
        _ => format!("can't run curl: {err}"),
    })?;
    if let (Some(body), Some(mut stdin)) = (&request.body, child.stdin.take()) {
        let body = body.clone();
        tokio::spawn(async move {
            let _ = stdin.write_all(body.as_bytes()).await;
        });
    }
    // Headers and body, and some room for the headers.
    let limit = (http::MAX_RESPONSE + (1 << 20)) as u64;
    let mut output = Vec::new();
    let mut stdout = child.stdout.take().expect("piped").take(limit + 1);
    let read = stdout.read_to_end(&mut output);
    let mut errors = String::new();
    let mut stderr = child.stderr.take().expect("piped").take(64 * 1024);
    let errors_read = stderr.read_to_string(&mut errors);
    let deadline = Duration::from_secs(request.timeout as u64 + 5);
    let (read, _) = tokio::time::timeout(deadline, async { tokio::join!(read, errors_read) })
        .await
        .map_err(|_| "the request timed out".to_string())?;
    read.map_err(|err| format!("reading the response: {err}"))?;
    if output.len() as u64 > limit {
        return Err(format!(
            "the response is larger than {} MiB",
            http::MAX_RESPONSE >> 20
        ));
    }
    let status = child
        .wait()
        .await
        .map_err(|err| format!("curl failed: {err}"))?;
    if !status.success() {
        let message = errors.trim().trim_start_matches("curl: ");
        return Err(if message.is_empty() {
            format!("the request failed ({status})")
        } else {
            message.to_string()
        });
    }
    parse(&output)
}

/// A public address of the request's host, the one curl must use; none
/// when it doesn't resolve but a proxy will.
async fn pinned_address(request: &HttpRequest) -> Result<Option<std::net::IpAddr>, String> {
    let refused = || {
        format!(
            "{} resolves to addresses plugins may not reach (not public)",
            request.host
        )
    };
    if let Ok(ip) = request.host.parse::<std::net::IpAddr>() {
        return http::is_public(ip).then_some(Some(ip)).ok_or_else(refused);
    }
    let addresses: Vec<std::net::IpAddr> =
        match tokio::net::lookup_host((request.host.as_str(), request.port)).await {
            Ok(addresses) => addresses.map(|address| address.ip()).collect(),
            Err(_) if proxied() => return Ok(None),
            Err(err) => return Err(format!("can't resolve {}: {err}", request.host)),
        };
    addresses
        .into_iter()
        .find(|ip| http::is_public(*ip))
        .map(Some)
        .ok_or_else(refused)
}

/// curl's `--include` output: status line, headers, blank line, body.
fn parse(output: &[u8]) -> Result<String, String> {
    let mut rest = output;
    loop {
        let end = rest
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or("no response headers")?;
        let head = String::from_utf8_lossy(&rest[..end]);
        let body = &rest[end + 4..];
        let mut lines = head.split("\r\n");
        let status: u16 = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .ok_or("invalid response status")?;
        // `100 Continue` and the like come before the response.
        if (100..200).contains(&status) {
            rest = body;
            continue;
        }
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
            })
            .collect();
        return Ok(http::response_json(status, &headers, body));
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn responses_are_parsed() {
        let reply = super::parse(
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/2 201 \r\nContent-Type: text/plain\r\nX-A: b: c\r\n\r\nhello\r\n\r\nworld",
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(json["status"], 201);
        assert_eq!(json["headers"][0][0], "content-type");
        assert_eq!(json["headers"][1][1], "b: c");
        assert_eq!(json["body"], "hello\r\n\r\nworld");
        assert!(super::parse(b"garbage").is_err());
    }
}
