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
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    task::{self, Poll},
    time::{Duration, Instant},
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
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, UnixListener},
    sync::{Notify, Semaphore},
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
/// How long a connection has to have said and been told nothing before it
/// may be closed to make room: when every place is taken, the one that has
/// been silent longest gives its place to the one that has just come. A
/// browser whose connection went that way opens another with its next request.
const QUIET: Duration = Duration::from_secs(10);

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

/// The connections that are open: where each came from, and when anything
/// last passed over it.
struct Open {
    held: Mutex<HashMap<u64, Held>>,
    /// Told the connections apart.
    next: AtomicU64,
    /// When this began to count. Times are kept as milliseconds since.
    began: Instant,
}

/// One open connection, as the count keeps it.
struct Held {
    from: IpAddr,
    heard: Arc<AtomicU64>,
    leave: Arc<Notify>,
}

/// One open connection's place in that count, given back when it ends.
struct Place<'a> {
    open: &'a Open,
    id: u64,
    /// When anything last passed over it, written by the connection itself.
    heard: Arc<AtomicU64>,
    /// Rung when the place has been given to another: the connection is to end.
    leave: Arc<Notify>,
}

impl Default for Open {
    fn default() -> Self {
        Self {
            held: Mutex::default(),
            next: AtomicU64::new(0),
            began: Instant::now(),
        }
    }
}

impl Open {
    /// How long this has counted, in milliseconds.
    fn now(&self) -> u64 {
        u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Takes a place for a connection from `from`, if there is one to take.
    fn enter(&self, from: IpAddr) -> Option<Place<'_>> {
        self.enter_at(from, self.now())
    }

    /// The same, at a time that is given.
    ///
    /// One address has its share and no more. When all the places are taken,
    /// whoever they are taken by, the connection that has been silent longest
    /// makes way, if it has been silent for [`QUIET`]: so that leaving
    /// connections open and saying nothing keeps nobody else out.
    fn enter_at(&self, from: IpAddr, now: u64) -> Option<Place<'_>> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let from_there = held.values().filter(|held| held.from == from).count();
        // Where the Gate stands in for whoever comes through it, its one
        // address is everybody on the internet.
        let most = match tunnel::is_gate(from) {
            true => OPEN,
            false => OPEN_FROM_ONE,
        };
        if from_there >= most as usize {
            return None;
        }
        if held.len() >= OPEN as usize {
            let quiet = u64::try_from(QUIET.as_millis()).unwrap_or(u64::MAX);
            let (longest, heard) = held
                .iter()
                .map(|(id, held)| (*id, held.heard.load(Ordering::Relaxed)))
                .min_by_key(|(id, heard)| (*heard, *id))?;
            if now.saturating_sub(heard) < quiet {
                return None;
            }
            if let Some(gone) = held.remove(&longest) {
                gone.leave.notify_one();
            }
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (heard, leave) = (Arc::new(AtomicU64::new(now)), Arc::new(Notify::new()));
        held.insert(
            id,
            Held {
                from,
                heard: Arc::clone(&heard),
                leave: Arc::clone(&leave),
            },
        );
        Some(Place {
            open: self,
            id,
            heard,
            leave,
        })
    }
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        let mut held = self
            .open
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        held.remove(&self.id);
    }
}

/// A connection that writes down when anything passes over it, either way.
struct Heard<S> {
    stream: S,
    heard: Arc<AtomicU64>,
    began: Instant,
}

impl<S> Heard<S> {
    fn now(&self) {
        let now = u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.heard.store(now, Ordering::Relaxed);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Heard<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut task::Context<'_>,
        into: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = into.filled().len();
        let read = Pin::new(&mut self.stream).poll_read(context, into);
        if into.filled().len() > before {
            self.now();
        }
        read
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Heard<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut task::Context<'_>,
        from: &[u8],
    ) -> Poll<io::Result<usize>> {
        let written = Pin::new(&mut self.stream).poll_write(context, from);
        if matches!(written, Poll::Ready(Ok(some)) if some > 0) {
            self.now();
        }
        written
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
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
    let Some(place) = open.enter(from.0) else {
        return;
    };
    let served = async {
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
        // What passes from here on is what the browser and the panel say to
        // each other, and each time it does is written down for the count.
        let stream = Heard {
            stream,
            heard: Arc::clone(&place.heard),
            began: open.began,
        };
        stream.now();
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
    };
    // Served until it ends, or until its place is given to another.
    tokio::select! {
        () = served => {}
        () = place.leave.notified() => {}
    }
}

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, KeyPair, date_time_ymd};

    use axum::http::{Request, header::HOST};

    use std::net::{IpAddr, Ipv4Addr};

    use super::{Heard, OPEN, OPEN_FROM_ONE, Open, QUIET, Shown, element, named, time, validity};

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
        assert!(open.held.lock().unwrap().is_empty());

        // The Gate's own address, where it stands in for everybody, has the
        // whole of the room and no more than that.
        let gate = IpAddr::V4(Ipv4Addr::new(10, 213, 77, 1));
        let all: Vec<_> = (0..OPEN).map(|_| open.enter(gate).unwrap()).collect();
        assert!(open.enter(gate).is_none());
        assert!(open.enter(one).is_none());
        drop(all);
        assert!(open.enter(one).is_some());
    }

    #[tokio::test]
    async fn when_every_place_is_taken_the_connection_silent_longest_makes_way() {
        use std::{sync::atomic::Ordering, time::Duration};

        use tokio::time::timeout;

        let open = Open::default();
        // Sixteen addresses, each with its share, at one moment: the room is full.
        let from = |place: u32| {
            let first = u32::from(Ipv4Addr::new(203, 0, 113, 0));
            IpAddr::V4(Ipv4Addr::from(first + place / OPEN_FROM_ONE))
        };
        let places: Vec<_> = (0..OPEN)
            .map(|place| open.enter_at(from(place), 1_000).unwrap())
            .collect();
        let other = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));
        let quiet = u64::try_from(QUIET.as_millis()).unwrap();
        // None of them has been silent for long: nobody makes way yet.
        assert!(open.enter_at(other, 1_000 + quiet - 1).is_none());
        // The first of them says something. The rest go on saying nothing.
        places[0].heard.store(1_000 + quiet, Ordering::Relaxed);
        let came = open.enter_at(other, 1_000 + quiet).unwrap();
        {
            let held = open.held.lock().unwrap();
            assert_eq!(held.len(), OPEN as usize);
            assert!(held.contains_key(&came.id));
            // The one that spoke stays, and the next, which did not, has gone.
            assert!(held.contains_key(&places[0].id));
            assert!(!held.contains_key(&places[1].id));
            assert!(held.contains_key(&places[2].id));
        }
        // It is told to end, and no other is.
        let told =
            |place: usize| timeout(Duration::from_millis(50), places[place].leave.notified());
        assert!(told(1).await.is_ok());
        assert!(told(0).await.is_err());
        assert!(told(2).await.is_err());
        // An address that has its share gets no more by it, full room or not.
        assert!(
            open.enter_at(from(OPEN_FROM_ONE), 1_000 + 2 * quiet)
                .is_none()
        );
        // What has made way gives nothing back twice when it ends.
        drop(places);
        assert_eq!(open.held.lock().unwrap().len(), 1);
        drop(came);
        assert!(open.held.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn what_passes_over_a_connection_is_written_down_as_it_passes() {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicU64, Ordering},
            },
            time::{Duration, Instant},
        };

        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (near, mut far) = tokio::io::duplex(64);
        let heard = Arc::new(AtomicU64::new(0));
        let began = Instant::now() - Duration::from_secs(60);
        let mut near = Heard {
            stream: near,
            heard: Arc::clone(&heard),
            began,
        };
        // Written to: the moment is kept.
        near.write_all(b"hello").await.unwrap();
        let written = heard.swap(0, Ordering::Relaxed);
        assert!(written >= 60_000, "{written}");
        // Read from, the same; and nothing is kept of a read that brought nothing.
        far.write_all(b"hi").await.unwrap();
        let mut some = [0u8; 2];
        near.read_exact(&mut some).await.unwrap();
        assert!(heard.swap(0, Ordering::Relaxed) >= 60_000);
        drop(far);
        assert_eq!(near.read(&mut some).await.unwrap(), 0);
        assert_eq!(heard.load(Ordering::Relaxed), 0);
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
