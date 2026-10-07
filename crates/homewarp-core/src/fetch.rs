//! Text from the internet, fetched on the owner's word (PLAN.md §11, Phase 6):
//! an egg from the address it is published at, and the list of eggs there are.
//! And, since Phase 7, a notice sent to an address the owner gave (`notify`),
//! which is held to the same.
//!
//! Core sits inside a home network, and an address is somebody else's text.
//! So what is fetched is held to what an egg's address looks like: `https`, on
//! the usual port, by a name. The name has to lead to an address on the
//! internet and to none inside a private network, and it is that address which
//! is then connected to, not whatever the name says a moment later. What comes
//! back is taken up to a set size and within a set time, and a redirection is
//! held to all of the same again.

use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::http::{
    Request, StatusCode, Uri,
    header::{ACCEPT, CONTENT_TYPE, HOST, LOCATION, USER_AGENT},
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{body::Bytes, client::conn::http1};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, crypto::ring, pki_types::ServerName};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{TlsConnector, client::TlsStream};

/// How long one fetch may take, redirections and all.
const PATIENCE: Duration = Duration::from_secs(20);
/// How long one address is given to answer a connection before the next is tried.
const CONNECTING: Duration = Duration::from_secs(6);
/// How many times an address may send on to another.
const REDIRECTIONS: usize = 3;
/// What Core says it is. Some hosts answer nobody who does not say.
const AGENT: &str = concat!("Homewarp/", env!("CARGO_PKG_VERSION"));

/// Why something was not fetched, as a sentence for whoever asked.
pub(crate) type Unfetched = String;

/// Where an address leads: the name, and what is asked of it.
#[derive(Debug, PartialEq, Eq)]
struct Place {
    host: String,
    path: String,
}

/// Reads an address, and refuses what an egg's address is not.
fn place(url: &str) -> Result<Place, Unfetched> {
    let refused = |sentence: &str| Err(sentence.to_owned());
    let url = url.trim();
    if url.is_empty() {
        return refused("That is not an address.");
    }
    // Said first, and of anything that does not begin so: an address typed
    // without its beginning is the commonest slip.
    if !url.starts_with("https://") {
        return refused("Only addresses that begin with https:// are fetched.");
    }
    let Ok(uri) = url.parse::<Uri>() else {
        return refused("That is not an address.");
    };
    let authority = uri.authority().map_or("", |authority| authority.as_str());
    if authority.contains('@') {
        return refused("An address with a name and password in it is not fetched.");
    }
    if uri.port_u16().is_some_and(|port| port != 443) {
        return refused("Only the usual port is fetched from.");
    }
    let host = uri.host().unwrap_or_default().to_lowercase();
    // A name, and not an address: private addresses are not asked, and a
    // certificate is for a name.
    let named = !host.is_empty()
        && host.contains('.')
        && host.parse::<IpAddr>().is_err()
        && !host.starts_with('[')
        && host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'));
    if !named {
        return refused("An address has to name a site, such as raw.githubusercontent.com.");
    }
    Ok(Place {
        host,
        path: uri
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned(),
    })
}

/// Whether an address is one on the internet: not this machine, not a private
/// network, not one of the ranges that are set aside for other things.
fn public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [a, b, ..] = address.octets();
            !(address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
                || address.is_multicast()
                || address.is_documentation()
                // "This network", carriers' own networks, protocol assignments,
                // benchmarking, and what is reserved for the future.
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (18..20).contains(&b))
                || a >= 240)
        }
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(inside) => public(IpAddr::V4(inside)),
            // Addresses on the internet begin with 2 or 3. Everything else is
            // this machine, one link, one site, or set aside.
            None => (address.segments()[0] & 0xe000) == 0x2000 && !documentation(address),
        },
    }
}

fn documentation(address: Ipv6Addr) -> bool {
    address.segments()[0] == 0x2001 && address.segments()[1] == 0x0db8
}

/// Whom this machine trusts to say whose a name is, for what it connects to.
pub(crate) fn trusting() -> Result<Arc<ClientConfig>, Unfetched> {
    static TRUSTING: OnceLock<Result<Arc<ClientConfig>, Unfetched>> = OnceLock::new();
    TRUSTING
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            let (trusted, _) =
                roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
            if trusted == 0 {
                return Err(
                    "This machine has no list of certificate authorities to trust, so nothing can be fetched."
                        .to_owned(),
                );
            }
            let mut config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|error| error.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Ok(Arc::new(config))
        })
        .clone()
}

/// What one asking came to.
enum Answered {
    Text(Bytes),
    /// Sent on to another address.
    Elsewhere(String),
}

/// A connection to where a name leads, with TLS on it: made to an address
/// that was looked at first, and to no other.
async fn reach(host: &str) -> Result<TlsStream<TcpStream>, Unfetched> {
    let found: Vec<SocketAddr> = tokio::net::lookup_host((host, 443))
        .await
        .map_err(|_| format!("{host} could not be looked up."))?
        .collect();
    if found.is_empty() {
        return Err(format!("{host} has no address."));
    }
    // All of them, and not only the one that will be used: a name that leads
    // both to the internet and into a private network is up to something.
    if !found.iter().all(|address| public(address.ip())) {
        return Err(format!(
            "{host} leads to an address inside a private network, and nothing is fetched from there."
        ));
    }
    let mut stream = None;
    for address in found {
        if let Ok(Ok(connected)) = timeout(CONNECTING, TcpStream::connect(address)).await {
            stream = Some(connected);
            break;
        }
    }
    let stream = stream.ok_or_else(|| format!("{host} could not be reached."))?;
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| format!("{host} is not a name a certificate can be for."))?;
    TlsConnector::from(trusting()?)
        .connect(name, stream)
        .await
        .map_err(|error| format!("{host} did not show a certificate this machine trusts: {error}."))
}

/// What a site said to one request: its status, where it sends on to if it
/// does, and what it sent. That last is nothing where it was more than was
/// asked for, or was cut short.
pub(crate) struct Said {
    pub(crate) status: StatusCode,
    pub(crate) onward: Option<String>,
    pub(crate) sent: Option<Bytes>,
}

/// One request over a connection that is there, and what came back, up to
/// `at_most` bytes of it.
pub(crate) async fn exchange<S, B>(
    stream: S,
    host: &str,
    request: Request<B>,
    at_most: usize,
) -> Result<Said, Unfetched>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let said =
        |error: &dyn std::fmt::Display| format!("{host} did not answer as a site does: {error}.");
    let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|error| said(&error))?;
    // The connection is driven for as long as the answer is being read.
    let driving = tokio::spawn(connection);
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| said(&error))?;
    let status = response.status();
    let onward = response
        .headers()
        .get(LOCATION)
        .and_then(|to| to.to_str().ok())
        .map(str::to_owned);
    let sent = Limited::new(response.into_body(), at_most)
        .collect()
        .await
        .ok()
        .map(|body| body.to_bytes());
    driving.abort();
    Ok(Said {
        status,
        onward,
        sent,
    })
}

/// Asks one address once, having made sure of where its name leads.
async fn ask(place: &Place, at_most: usize) -> Result<Answered, Unfetched> {
    let host = place.host.as_str();
    let stream = reach(host).await?;
    let request = Request::get(place.path.as_str())
        .header(HOST, host)
        .header(USER_AGENT, AGENT)
        .header(ACCEPT, "*/*")
        .body(Full::<Bytes>::default())
        .map_err(|error| format!("{host} cannot be asked for that: {error}."))?;
    let said = exchange(stream, host, request, at_most).await?;
    let status = said.status;
    if status.is_redirection() {
        let to = said
            .onward
            .ok_or_else(|| format!("{host} sent on to nowhere."))?;
        // To another place on the same site, or to another site altogether.
        Ok(Answered::Elsewhere(match to.starts_with('/') {
            true => format!("https://{host}{to}"),
            false => to,
        }))
    } else if status == StatusCode::OK {
        said.sent.map(Answered::Text).ok_or_else(|| {
            format!(
                "What is at that address is more than {} kB, or was cut short.",
                at_most / 1024
            )
        })
    } else if status == StatusCode::NOT_FOUND {
        Err(format!("{host} has nothing at that address."))
    } else if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        Err(format!(
            "{host} would not answer just now ({status}). It may have been asked too often: try again in a while."
        ))
    } else {
        Err(format!("{host} answered {status}."))
    }
}

/// How much of an answer to a notice is read. What it says is of no use here.
const ANSWER_TO_A_NOTICE: usize = 4096;

/// Sends JSON over a connection that is there, as a notice is sent, and says
/// whether the site took it.
async fn deliver<S>(stream: S, place: &Place, json: &serde_json::Value) -> Result<(), Unfetched>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let host = place.host.as_str();
    let request = Request::post(place.path.as_str())
        .header(HOST, host)
        .header(USER_AGENT, AGENT)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(json.to_string())))
        .map_err(|error| format!("{host} cannot be sent that: {error}."))?;
    let status = exchange(stream, host, request, ANSWER_TO_A_NOTICE)
        .await?
        .status;
    if status.is_success() {
        Ok(())
    } else if status.is_redirection() {
        // Followed, a notice would go to an address nobody chose.
        Err(format!(
            "{host} sends on to another address ({status}). Give the address it sends on to."
        ))
    } else if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ) {
        Err(format!(
            "{host} did not take it ({status}): that address is not one it takes notices at, or no longer is."
        ))
    } else {
        Err(format!("{host} did not take it ({status})."))
    }
}

/// The site an address names, if it is an address that is fetched from at all.
pub(crate) fn site(url: &str) -> Result<String, Unfetched> {
    Ok(place(url)?.host)
}

/// Sends JSON to an address, as a notice is sent to Discord, Slack and their
/// like. It is held to everything a fetch is held to, and is sent to the one
/// address: not on to another.
pub(crate) async fn send_json(url: &str, json: &serde_json::Value) -> Result<(), Unfetched> {
    let sent = async {
        let place = place(url)?;
        let stream = reach(&place.host).await?;
        deliver(stream, &place, json).await
    };
    match timeout(PATIENCE, sent).await {
        Ok(sent) => sent,
        Err(_) => Err("That address took too long to answer.".to_owned()),
    }
}

/// What is at an address, up to `at_most` bytes of it, within `patience`,
/// redirections and all. A file takes longer than a page of text does.
pub(crate) async fn bytes(
    url: &str,
    at_most: usize,
    patience: Duration,
) -> Result<Bytes, Unfetched> {
    let fetched = async {
        let mut place = place(url)?;
        for _ in 0..=REDIRECTIONS {
            match ask(&place, at_most).await? {
                Answered::Text(bytes) => return Ok(bytes),
                Answered::Elsewhere(to) => place = self::place(&to)?,
            }
        }
        Err("That address sends on and on to others.".to_owned())
    };
    match timeout(patience, fetched).await {
        Ok(fetched) => fetched,
        Err(_) => Err("That address took too long to answer.".to_owned()),
    }
}

/// A file of a release, from where this Homewarp's releases are: an address
/// its owner gave it when it was installed, and not one that was typed into a
/// page. So it may be a plain `http://` one, on a port and at an address of
/// any kind: a release served on the owner's own network. Whatever comes from
/// there is believed for its signature and not for where it came from.
pub(crate) async fn released(
    url: &str,
    at_most: usize,
    patience: Duration,
) -> Result<Bytes, Unfetched> {
    let Some(rest) = url.trim().strip_prefix("http://") else {
        return bytes(url, at_most, patience).await;
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_owned()),
    };
    let fetched = async {
        let at = match authority.rsplit_once(':') {
            Some((_, port)) if port.parse::<u16>().is_ok() => authority.to_owned(),
            _ => format!("{authority}:80"),
        };
        let stream = timeout(CONNECTING, TcpStream::connect(at.as_str()))
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| format!("{authority} could not be reached."))?;
        let request = Request::get(path.as_str())
            .header(HOST, authority)
            .header(USER_AGENT, AGENT)
            .header(ACCEPT, "*/*")
            .body(Full::<Bytes>::default())
            .map_err(|error| format!("{authority} cannot be asked for that: {error}."))?;
        let said = exchange(stream, authority, request, at_most).await?;
        match said.status {
            StatusCode::OK => said.sent.ok_or_else(|| {
                format!(
                    "What is at that address is more than {} kB, or was cut short.",
                    at_most / 1024
                )
            }),
            StatusCode::NOT_FOUND => Err(format!("{authority} has nothing at that address.")),
            status => Err(format!("{authority} answered {status}.")),
        }
    };
    match timeout(patience, fetched).await {
        Ok(fetched) => fetched,
        Err(_) => Err("That address took too long to answer.".to_owned()),
    }
}

/// The text at an address, up to `at_most` bytes of it.
pub(crate) async fn text(url: &str, at_most: usize) -> Result<String, Unfetched> {
    let fetched = bytes(url, at_most, PATIENCE).await?;
    String::from_utf8(fetched.to_vec())
        .map_err(|_| "What is at that address is not text.".to_owned())
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{Place, deliver, place, public, send_json, site, text};

    #[test]
    fn an_address_is_https_on_the_usual_port_and_by_a_name() {
        assert_eq!(
            place(
                " https://raw.githubusercontent.com/pelican-eggs/minecraft/main/java/paper/egg-paper.yaml?x=1 "
            ),
            Ok(Place {
                host: "raw.githubusercontent.com".to_owned(),
                path: "/pelican-eggs/minecraft/main/java/paper/egg-paper.yaml?x=1".to_owned(),
            })
        );
        assert_eq!(
            place("https://Example.COM:443").unwrap().host,
            "example.com"
        );
        assert_eq!(place("https://example.com").unwrap().path, "/");
        for (odd, why) in [
            ("http://example.com/egg.json", "https://"),
            ("ftp://example.com/egg.json", "https://"),
            ("example.com/egg.json", "https://"),
            ("file:///etc/passwd", "https://"),
            ("https://example.com:8443/egg.json", "usual port"),
            (
                "https://user:secret@example.com/egg.json",
                "name and password",
            ),
            ("https://192.168.1.1/egg.json", "name a site"),
            ("https://[::1]/egg.json", "name a site"),
            ("https://localhost/egg.json", "name a site"),
            ("https://2130706433/egg.json", "name a site"),
            ("https://exa_mple.com/egg.json", "name a site"),
            ("not an address at all", "https://"),
            ("https://", "not an address"),
            ("https://exa mple.com/", "not an address"),
            ("", "not an address"),
        ] {
            let refused = place(odd).unwrap_err();
            assert!(refused.contains(why), "{odd}: {refused}");
        }
    }

    #[test]
    fn only_addresses_on_the_internet_are_fetched_from() {
        for outside in [
            "140.82.112.3",
            "185.199.108.133",
            "1.1.1.1",
            "2606:50c0:8000::154",
            "::ffff:8.8.8.8",
        ] {
            assert!(public(outside.parse::<IpAddr>().unwrap()), "{outside}");
        }
        for inside in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.50",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "192.0.0.8",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd7a:115c:a1e0::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:192.168.1.1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!public(inside.parse::<IpAddr>().unwrap()), "{inside}");
        }
    }

    /// A site at the other end of a connection: reads one request to its end,
    /// answers with `status`, and says what it was sent.
    async fn site_answering(mut theirs: tokio::io::DuplexStream, status: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut got = Vec::new();
        let mut some = [0u8; 1024];
        loop {
            let read = theirs.read(&mut some).await.unwrap();
            got.extend_from_slice(&some[..read]);
            let text = String::from_utf8_lossy(&got);
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length: ")?
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0);
                if body.len() >= length {
                    break;
                }
            }
            assert!(read > 0, "the request ended before it was whole");
        }
        let answer = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
        theirs.write_all(answer.as_bytes()).await.unwrap();
        String::from_utf8(got).unwrap()
    }

    #[tokio::test]
    async fn a_notice_is_sent_as_json_to_the_one_address() {
        let place = place("https://discord.example/api/webhooks/1/s3cret").unwrap();
        let json = serde_json::json!({ "content": "Lobby crashed (exit code 1)." });

        let (ours, theirs) = tokio::io::duplex(4096);
        let site = tokio::spawn(site_answering(theirs, "204 No Content"));
        assert_eq!(deliver(ours, &place, &json).await, Ok(()));
        let sent = site.await.unwrap();
        let (head, body) = sent.split_once("\r\n\r\n").unwrap();
        let head = head.to_lowercase();
        assert!(
            head.starts_with("post /api/webhooks/1/s3cret http/1.1\r\n"),
            "{head}"
        );
        assert!(head.contains("\r\nhost: discord.example"), "{head}");
        assert!(
            head.contains("\r\ncontent-type: application/json"),
            "{head}"
        );
        assert!(
            head.contains(&format!("\r\ncontent-length: {}", body.len())),
            "{head}"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            json
        );

        // A site that does not take it, and one that would send it on elsewhere.
        for (status, why) in [
            ("404 Not Found", "no longer is"),
            ("429 Too Many Requests", "did not take it (429"),
            ("500 Internal Server Error", "did not take it (500"),
            ("302 Found", "sends on to another address"),
        ] {
            let (ours, theirs) = tokio::io::duplex(4096);
            let site = tokio::spawn(site_answering(theirs, status));
            let refused = deliver(ours, &place, &json).await.unwrap_err();
            assert!(refused.contains(why), "{status}: {refused}");
            site.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_notice_is_held_to_what_a_fetch_is_held_to() {
        let json = serde_json::json!({ "content": "x" });
        for (url, why) in [
            ("http://example.com/hook", "https://"),
            ("https://192.168.1.10/hook", "name a site"),
            ("https://example.com:8443/hook", "usual port"),
        ] {
            let refused = send_json(url, &json).await.unwrap_err();
            assert!(refused.contains(why), "{url}: {refused}");
        }
        assert_eq!(
            site("https://Discord.com/api/webhooks/1/x").unwrap(),
            "discord.com"
        );
    }

    #[tokio::test]
    async fn a_name_that_leads_into_this_machine_is_not_asked() {
        // No network is needed to refuse these: they are refused before any is used.
        for (url, why) in [
            ("http://example.com/", "https://"),
            ("https://10.0.0.1/", "name a site"),
        ] {
            let refused = text(url, 1024).await.unwrap_err();
            assert!(refused.contains(why), "{url}: {refused}");
        }
    }
}
