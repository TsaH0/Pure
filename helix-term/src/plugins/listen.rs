//! `listener.listen`: plugins receiving HTTP requests on 127.0.0.1, for
//! example problems from the Competitive Companion browser extension.
//!
//! Only `POST` requests with `Content-Type: application/json` and a `Host`
//! of `localhost` or `127.0.0.1` are passed on. A web page can't send such a
//! request without a CORS preflight (refused here), and a page that rebinds
//! its own domain to 127.0.0.1 still sends its domain as `Host`.

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use helix_plugin::{Callback, MAX_LISTEN_BODY};
use parking_lot::Mutex;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};

helix_event::runtime_local! {
    /// Running listeners: `(plugin, id, task)`.
    static LISTENERS: Mutex<Vec<(String, u32, tokio::task::AbortHandle)>> = Mutex::new(Vec::new());
}

/// Longest request line and headers.
const MAX_HEAD: usize = 16 << 10;
/// How long one connection may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Most connections one listener reads at once.
const MAX_CONNECTIONS: usize = 16;

/// Listen on 127.0.0.1:`port` for request `id` of `plugin`.
pub fn start(plugin: String, id: u32, port: u16) {
    // Held while spawning, so a listener that fails at once can't remove
    // itself before it is added.
    let mut listeners = LISTENERS.lock();
    let owner = plugin.clone();
    let task = tokio::spawn(async move {
        let listener = match bind(port).await {
            Ok(listener) => listener,
            Err(error) => {
                LISTENERS
                    .lock()
                    .retain(|(owner, listener, _)| (owner, *listener) != (&plugin, id));
                let error = format!("can't listen on 127.0.0.1:{port}: {error}");
                super::deliver_async(plugin, Callback::ListenFailed { id, error }).await;
                return;
            }
        };
        let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // Out of file descriptors and the like: try again later.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            // Too many at once: drop this one.
            let Ok(permit) = connections.clone().try_acquire_owned() else {
                continue;
            };
            let plugin = plugin.clone();
            tokio::spawn(async move {
                let request = tokio::time::timeout(REQUEST_TIMEOUT, receive(stream)).await;
                drop(permit);
                if let Ok(Some(body)) = request {
                    super::deliver_async(plugin, Callback::ListenRequest { id, body }).await;
                }
            });
        }
    });
    listeners.push((owner, id, task.abort_handle()));
}

/// Bind 127.0.0.1:`port`. A listener stopped just before (by a reload)
/// may still hold the port for a moment, so retry briefly while it is in
/// use.
async fn bind(port: u16) -> std::io::Result<TcpListener> {
    let mut tries = 0;
    loop {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse && tries < 5 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            result => return result,
        }
    }
}

/// Stop listener `id` of `plugin`.
pub fn stop(plugin: &str, id: u32) {
    LISTENERS.lock().retain(|(owner, listener, task)| {
        let stopped = (owner.as_str(), *listener) == (plugin, id);
        if stopped {
            task.abort();
        }
        !stopped
    });
}

/// Stop the listeners of `plugin` (`None`: all of them).
pub fn stop_plugin(plugin: Option<&str>) {
    LISTENERS.lock().retain(|(owner, _, task)| {
        let keep = plugin.is_some_and(|plugin| plugin != owner);
        if !keep {
            task.abort();
        }
        keep
    });
}

/// Read one request from `stream` and answer it; the body if it is one to
/// pass on.
async fn receive(mut stream: TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if buffer.len() > MAX_HEAD {
            return respond(&mut stream, "431 Request Header Fields Too Large").await;
        }
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&buffer[..head_end]).ok()?;
    let request = match parse_head(head) {
        Ok(request) => request,
        Err(status) => return respond(&mut stream, status).await,
    };
    if request.length > MAX_LISTEN_BODY {
        return respond(&mut stream, "413 Content Too Large").await;
    }
    let mut body = buffer.split_off(head_end + 4);
    body.truncate(request.length);
    while body.len() < request.length {
        let mut chunk = vec![0; (request.length - body.len()).min(64 << 10)];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    let Ok(body) = String::from_utf8(body) else {
        return respond(&mut stream, "400 Bad Request").await;
    };
    respond(&mut stream, "200 OK").await;
    Some(body)
}

async fn respond(stream: &mut TcpStream, status: &str) -> Option<String> {
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    None
}

#[derive(Debug, PartialEq, Eq)]
struct Head {
    length: usize,
}

/// Check a request's line and headers; the response status if it is
/// refused.
fn parse_head(head: &str) -> Result<Head, &'static str> {
    let mut lines = head.split("\r\n");
    let method = lines
        .next()
        .and_then(|line| line.split(' ').next())
        .ok_or("400 Bad Request")?;
    if method != "POST" {
        return Err("405 Method Not Allowed");
    }
    let (mut length, mut json, mut host) = (None, false, false);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err("400 Bad Request");
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = Some(value.parse().map_err(|_| "400 Bad Request")?),
            "transfer-encoding" => return Err("411 Length Required"),
            "content-type" => {
                json = value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
            }
            "host" => {
                let name = value.rsplit_once(':').map_or(value, |(name, _)| name);
                host = name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1";
            }
            _ => {}
        }
    }
    if !host {
        return Err("403 Forbidden");
    }
    if !json {
        return Err("415 Unsupported Media Type");
    }
    Ok(Head {
        length: length.ok_or("411 Length Required")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(lines: &[&str]) -> Result<Head, &'static str> {
        parse_head(&lines.join("\r\n"))
    }

    #[test]
    fn accepts_what_competitive_companion_sends() {
        let request = head(&[
            "POST / HTTP/1.1",
            "Host: localhost:27121",
            "Content-Type: application/json",
            "Content-Length: 12",
        ]);
        assert_eq!(request, Ok(Head { length: 12 }));
        let request = head(&[
            "POST / HTTP/1.1",
            "host: 127.0.0.1:27121",
            "content-type: application/json; charset=utf-8",
            "content-length: 0",
        ]);
        assert_eq!(request, Ok(Head { length: 0 }));
    }

    #[test]
    fn refuses_what_a_web_page_can_send() {
        // A preflight, a GET, a "simple" form post and a rebound domain.
        let preflight = head(&["OPTIONS / HTTP/1.1", "Host: localhost:27121"]);
        assert_eq!(preflight, Err("405 Method Not Allowed"));
        assert_eq!(
            head(&["GET / HTTP/1.1", "Host: localhost"]),
            Err("405 Method Not Allowed")
        );
        let simple = head(&[
            "POST / HTTP/1.1",
            "Host: localhost:27121",
            "Content-Type: text/plain",
            "Content-Length: 2",
        ]);
        assert_eq!(simple, Err("415 Unsupported Media Type"));
        let rebound = head(&[
            "POST / HTTP/1.1",
            "Host: evil.example:27121",
            "Content-Type: application/json",
            "Content-Length: 2",
        ]);
        assert_eq!(rebound, Err("403 Forbidden"));
        let no_host = head(&["POST / HTTP/1.1", "Content-Type: application/json"]);
        assert_eq!(no_host, Err("403 Forbidden"));
        let chunked = head(&[
            "POST / HTTP/1.1",
            "Host: localhost",
            "Content-Type: application/json",
            "Transfer-Encoding: chunked",
        ]);
        assert_eq!(chunked, Err("411 Length Required"));
    }
}
