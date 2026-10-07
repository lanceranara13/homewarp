//! The panel over TLS, ended at home (PLAN.md §5.9; §11, Phase 5).
//!
//! The VPS forwards the panel's port as it forwards a server's, and sees what
//! anyone on the way sees: who is talking, and nothing of what is said. The
//! key never leaves this machine. Connections arrive through a door, as the
//! panel's others do, and each is taken up in a task of its own, so that a
//! handshake nobody finishes holds nobody else up.

use std::{
    collections::HashMap,
    fmt, io,
    net::IpAddr,
    path::Path,
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::Duration,
};

use anyhow::Context;
use axum::{
    Router,
    extract::ConnectInfo,
    http::{HeaderValue, Request, header::HOST},
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use rustls::{
    ServerConfig,
    crypto::ring,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, UnixListener},
    sync::Semaphore,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

use crate::{
    api::{self, AppState},
    clock,
    door::{self, Client},
    tunnel,
};

/// A door sends its line at once. One that has not come by now is not coming.
const PATIENCE: Duration = Duration::from_secs(2);
/// How long a browser has to finish shaking hands, and then to say what it wants.
const HANDSHAKE: Duration = Duration::from_secs(10);
const REQUEST: Duration = Duration::from_secs(30);
/// How many may be shaking hands at once. One more is turned away unheard:
/// this port is open to the internet, and a handshake is work.
const SHAKING: usize = 256;
/// How many connections may be open at once, in all and from one address. A
/// connection that asks nothing costs little, and nothing else limits how
/// many of them anyone on the internet may leave open.
const OPEN: u32 = 1024;
const OPEN_FROM_ONE: u32 = 64;

/// Put on a request that came in over TLS, for whatever answers it.
#[derive(Clone, Copy)]
pub(crate) struct Secured;

/// The certificate the panel shows. It is changed for a newer one while
/// connections are being taken up, and nothing is started again for it.
#[derive(Default)]
pub(crate) struct Shown(RwLock<Option<Arc<CertifiedKey>>>);

impl Shown {
    /// Shows this certificate from now on: a chain and its key, both in PEM.
    pub(crate) fn show(&self, chain: &str, key: &str) -> anyhow::Result<()> {
        let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain.as_bytes())
            .collect::<Result<_, _>>()
            .context("reading the certificate")?;
        anyhow::ensure!(
            !chain.is_empty(),
            "there is no certificate in what was kept"
        );
        let key = PrivateKeyDer::from_pem_slice(key.as_bytes())
            .context("reading the certificate's key")?;
        let key = CertifiedKey::from_der(chain, key, &ring::default_provider())
            .context("the key is not the certificate's")?;
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(key));
        Ok(())
    }

    /// Shows none: handshakes fail until there is one again.
    pub(crate) fn hide(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl fmt::Debug for Shown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Shown")
    }
}

impl ResolvesServerCert for Shown {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The connections that are open, counted by where each came from.
#[derive(Default)]
struct Open(Mutex<HashMap<IpAddr, u32>>);

/// One open connection's place in that count, given back when it ends.
struct Place<'a> {
    open: &'a Open,
    from: IpAddr,
}

impl Open {
    /// Takes a place for a connection from `from`, if there is one to take.
    fn enter(&self, from: IpAddr) -> Option<Place<'_>> {
        let mut open = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let all: u32 = open.values().sum();
        let from_there = open.get(&from).copied().unwrap_or(0);
        // Where the Gate stands in for whoever comes through it, its one
        // address is everybody on the internet.
        let most = match tunnel::is_gate(from) {
            true => OPEN,
            false => OPEN_FROM_ONE,
        };
        if all >= OPEN || from_there >= most {
            return None;
        }
        open.insert(from, from_there + 1);
        Some(Place { open: self, from })
    }
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        let mut open = self.open.0.lock().unwrap_or_else(PoisonError::into_inner);
        match open.get_mut(&self.from) {
            Some(count) if *count > 1 => *count -= 1,
            _ => {
                open.remove(&self.from);
            }
        }
    }
}

/// One element of DER with this tag: what is in it, and what follows it.
fn element(der: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = der.split_first()?;
    let (&first, rest) = rest.split_first()?;
    if found != tag {
        return None;
    }
    let (length, rest) = match first {
        0..=0x7f => (usize::from(first), rest),
        // The length takes this many bytes of its own.
        0x81..=0x84 => {
            let (bytes, rest) = rest.split_at_checked(usize::from(first & 0x7f))?;
            let length = bytes
                .iter()
                .fold(0, |length, byte| (length << 8) | usize::from(*byte));
            (length, rest)
        }
        _ => return None,
    };
    rest.split_at_checked(length)
}

/// A time as a certificate writes it, in Unix seconds: `YYMMDDHHMMSSZ`, or
/// with a whole year for the dates the short form cannot say.
fn time(der: &[u8]) -> Option<(i64, &[u8])> {
    let tag = *der.first()?;
    let (text, rest) = element(der, tag)?;
    let text = std::str::from_utf8(text).ok()?.strip_suffix('Z')?;
    let (year, text) = match tag {
        0x17 => {
            let (year, text) = text.split_at_checked(2)?;
            let year: i64 = year.parse().ok()?;
            // Two digits are a year from 1950 to 2049.
            (if year < 50 { 2000 + year } else { 1900 + year }, text)
        }
        0x18 => {
            let (year, text) = text.split_at_checked(4)?;
            (year.parse().ok()?, text)
        }
        _ => return None,
    };
    if text.len() != 10 || !text.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let two = |at: usize| text[at..at + 2].parse::<i64>().ok();
    let days = clock::days(year, two(0)?, two(2)?);
    let seconds = ((days * 24 + two(4)?) * 60 + two(6)?) * 60 + two(8)?;
    Some((seconds, rest))
}

/// From when and until when the first certificate of a chain counts, in Unix
/// seconds. Read off the certificate itself, of which only the way to those
/// two dates is looked at: a version, a number, an algorithm and an issuer
/// come before them.
pub(crate) fn validity(chain: &str) -> Option<(i64, i64)> {
    let certificate = CertificateDer::pem_slice_iter(chain.as_bytes())
        .next()?
        .ok()?;
    let (certificate, _) = element(&certificate, 0x30)?;
    let (mut rest, _) = element(certificate, 0x30)?;
    if rest.first() == Some(&0xa0) {
        (_, rest) = element(rest, 0xa0)?;
    }
    (_, rest) = element(rest, 0x02)?;
    (_, rest) = element(rest, 0x30)?;
    (_, rest) = element(rest, 0x30)?;
    let (validity, _) = element(rest, 0x30)?;
    let (from, rest) = time(validity)?;
    let (until, _) = time(rest)?;
    Some((from, until))
}

/// Serves the panel over TLS where `listen` says: `unix:<path>` for a door to
/// pass connections to, or an address to listen on.
pub async fn serve(state: AppState, listen: String) -> anyhow::Result<()> {
    let mut config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(state.shown.clone());
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let app = api::app(state);
    let shaking = Arc::new(Semaphore::new(SHAKING));
    let open = Arc::new(Open::default());
    match listen.strip_prefix("unix:") {
        Some(socket) => {
            let socket = Path::new(socket);
            if let Some(folder) = socket.parent() {
                std::fs::create_dir_all(folder)?;
            }
            // One that a Core which stopped left behind.
            match std::fs::remove_file(socket) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
            let listener = UnixListener::bind(socket)?;
            tracing::info!("The panel is served over TLS on {}", socket.display());
            loop {
                let (mut stream, _) = listener.accept().await?;
                let (acceptor, app) = (acceptor.clone(), app.clone());
                let (shaking, open) = (Arc::clone(&shaking), Arc::clone(&open));
                tokio::spawn(async move {
                    // The door says first where the connection came from. One
                    // that says nothing is no door's, and is dropped.
                    let said = timeout(PATIENCE, door::announced(&mut stream)).await;
                    if let Ok(Ok(from)) = said {
                        connection(acceptor, stream, app, Client(from), &shaking, &open).await;
                    }
                });
            }
        }
        None => {
            let listener = TcpListener::bind(&listen).await?;
            tracing::info!("The panel is served over TLS on {listen}");
            loop {
                let (stream, from) = listener.accept().await?;
                let from = Client(from.ip().to_canonical());
                let (acceptor, app) = (acceptor.clone(), app.clone());
                let (shaking, open) = (Arc::clone(&shaking), Arc::clone(&open));
                tokio::spawn(async move {
                    connection(acceptor, stream, app, from, &shaking, &open).await;
                });
            }
        }
    }
}

/// Says, where the request does not, which site it was made to.
///
/// Over HTTP/2, which a browser speaks here, the name comes with the request's
/// address and there is no `Host` header. Whatever holds a request against the
/// site it was made to reads that header: the check that keeps other sites'
/// pages from making changes here, which would otherwise refuse this one's.
fn named<B>(request: &mut Request<B>) {
    if request.headers().contains_key(HOST) {
        return;
    }
    let host = request
        .uri()
        .authority()
        .and_then(|authority| HeaderValue::from_str(authority.as_str()).ok());
    if let Some(host) = host {
        request.headers_mut().insert(HOST, host);
    }
}

/// One connection: the handshake, and then the panel as it is served anywhere
/// else, with who is asking and that it came over TLS put on each request.
async fn connection(
    acceptor: TlsAcceptor,
    stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    app: Router,
    from: Client,
    shaking: &Semaphore,
    open: &Open,
) {
    // Held until the connection ends. One more than there is room for is
    // turned away before it has cost a handshake.
    let Some(_place) = open.enter(from.0) else {
        return;
    };
    let stream = {
        let Ok(_shaking) = shaking.try_acquire() else {
            return;
        };
        match timeout(HANDSHAKE, acceptor.accept(stream)).await {
            Ok(Ok(stream)) => stream,
            // A scanner, or a browser that does not trust the certificate.
            _ => return,
        }
    };
    let service = app.map_request(move |mut request: Request<_>| {
        request.extensions_mut().insert(ConnectInfo(from));
        request.extensions_mut().insert(Secured);
        named(&mut request);
        request
    });
    let mut serving = Builder::new(TokioExecutor::new());
    serving
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(REQUEST);
    // An error here is a connection that ended, which is how they all end.
    let _ = serving
        .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(service))
        .await;
}

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, KeyPair, date_time_ymd};

    use axum::http::{Request, header::HOST};

    use std::net::{IpAddr, Ipv4Addr};

    use super::{OPEN, OPEN_FROM_ONE, Open, Shown, element, named, time, validity};

    #[test]
    fn no_more_connections_are_open_than_there_is_room_for() {
        let open = Open::default();
        let one = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50));
        let mut places: Vec<_> = (0..OPEN_FROM_ONE)
            .map(|_| open.enter(one).unwrap())
            .collect();
        // One address has had its share, and another has not.
        assert!(open.enter(one).is_none());
        let other = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 51));
        let elsewhere = open.enter(other).unwrap();
        // A connection that ends gives its place back.
        places.pop();
        places.push(open.enter(one).unwrap());
        assert!(open.enter(one).is_none());
        drop(elsewhere);
        drop(places);
        assert!(open.0.lock().unwrap().is_empty());

        // The Gate's own address, where it stands in for everybody, has the
        // whole of the room and no more than that.
        let gate = IpAddr::V4(Ipv4Addr::new(10, 213, 77, 1));
        let all: Vec<_> = (0..OPEN).map(|_| open.enter(gate).unwrap()).collect();
        assert!(open.enter(gate).is_none());
        assert!(open.enter(one).is_none());
        drop(all);
        assert!(open.enter(one).is_some());
    }

    #[test]
    fn a_request_that_names_its_site_only_in_its_address_is_given_the_header() {
        // As HTTP/2 sends one: the name is part of the address.
        let mut request = Request::get("https://panel.example.com:8443/api/v1/session")
            .body(())
            .unwrap();
        named(&mut request);
        assert_eq!(request.headers()[HOST], "panel.example.com:8443");
        // As HTTP/1.1 sends one: the header is there, and is left as it is.
        let mut request = Request::get("/api/v1/session")
            .header(HOST, "panel.example.com:8443")
            .body(())
            .unwrap();
        named(&mut request);
        assert_eq!(request.headers()[HOST], "panel.example.com:8443");
        // And one that names none is not given one.
        let mut request = Request::get("/api/v1/session").body(()).unwrap();
        named(&mut request);
        assert!(!request.headers().contains_key(HOST));
    }

    #[test]
    fn a_certificate_says_how_long_it_counts_and_is_shown_with_its_own_key_only() {
        let mut asked = CertificateParams::new(vec!["panel.example.com".to_owned()]).unwrap();
        asked.not_before = date_time_ymd(2026, 10, 6);
        asked.not_after = date_time_ymd(2027, 1, 4);
        let key = KeyPair::generate().unwrap();
        let chain = asked.self_signed(&key).unwrap().pem();
        // Midnight on 2026-10-06, and ninety days on.
        assert_eq!(validity(&chain), Some((1_791_244_800, 1_799_020_800)));
        assert_eq!(validity("not a certificate"), None);

        let shown = Shown::default();
        shown.show(&chain, &key.serialize_pem()).unwrap();
        assert!(shown.0.read().unwrap().is_some());
        // Another key is not this certificate's, and nothing is no certificate.
        let other = KeyPair::generate().unwrap();
        assert!(shown.show(&chain, &other.serialize_pem()).is_err());
        assert!(shown.show("", &key.serialize_pem()).is_err());
        shown.hide();
        assert!(shown.0.read().unwrap().is_none());
    }

    #[test]
    fn a_certificates_dates_are_read_in_both_the_ways_they_are_written() {
        // 2026-10-06 15:30:45 UTC, with two digits for the year and with four.
        let short = [&[0x17, 13][..], b"261006153045Z", b"rest"].concat();
        assert_eq!(time(&short), Some((1_791_300_645, &b"rest"[..])));
        let long = [&[0x18, 15][..], b"20261006153045Z"].concat();
        assert_eq!(time(&long), Some((1_791_300_645, &b""[..])));
        // Two digits from 50 on are the last century's.
        let old = [&[0x17, 13][..], b"700101000000Z"].concat();
        assert_eq!(time(&old), Some((0, &b""[..])));
        for odd in [
            &b"\x17\x0d261006153045+"[..],
            b"\x17\x0b2610061530Z",
            b"\x0c\x0d261006153045Z",
            b"\x17\x0d26100615304",
            b"",
        ] {
            assert_eq!(time(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn an_element_is_as_long_as_it_says_and_no_longer_than_there_is() {
        assert_eq!(
            element(&[0x30, 2, 1, 2, 3], 0x30),
            Some((&[1, 2][..], &[3][..]))
        );
        // A length too long for one byte says how many bytes it takes.
        let long = [&[0x30, 0x82, 0x01, 0x00][..], &[7; 256], &[9]].concat();
        let (inside, after) = element(&long, 0x30).unwrap();
        assert_eq!((inside.len(), after), (256, &[9][..]));
        assert_eq!(element(&[0x30, 5, 1, 2], 0x30), None);
        assert_eq!(element(&[0x31, 1, 1], 0x30), None);
        assert_eq!(element(&[0x30, 0x80, 1], 0x30), None);
        assert_eq!(element(&[0x30], 0x30), None);
    }
}
