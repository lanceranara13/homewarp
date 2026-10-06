//! What a door tells Core of who came through it (PLAN.md §10: the door; §11,
//! Phase 5).
//!
//! Core listens on a socket in the file system, and a door passes it each
//! connection that arrives at the panel's port. To Core every one of them
//! would come from the door. So the door sends a line ahead of each, saying
//! where it came from: the first line of the PROXY protocol, which is the
//! usual way of saying so. Only a door can connect to that socket, so the line
//! is believed.

use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, connect_info::Connected},
    http::request::Parts,
    serve::{IncomingStream, Listener},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::{TcpListener, UnixListener, UnixStream},
    time::timeout,
};

/// The longest such a line is, as the protocol has it.
const LONGEST: usize = 107;
/// A door sends its line at once. One that has not come by now is not coming.
const PATIENCE: Duration = Duration::from_secs(2);

/// The address a connection came from: a player's or a browser's own, not the
/// door's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Client(pub IpAddr);

/// The line a door sends ahead of a connection from `from` that arrived at `at`.
pub fn announce(from: SocketAddr, at: SocketAddr) -> String {
    // The two have to be of one family for the line to say so.
    match (from.ip().to_canonical(), at.ip().to_canonical()) {
        (IpAddr::V4(from_ip), IpAddr::V4(at_ip)) => format!(
            "PROXY TCP4 {from_ip} {at_ip} {} {}\r\n",
            from.port(),
            at.port()
        ),
        (from_ip, _) => format!("PROXY TCP6 {from_ip} :: {} {}\r\n", from.port(), at.port()),
    }
}

/// Reads that line off the start of a connection, and no more than it: what
/// follows is the connection itself.
pub async fn announced(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<IpAddr> {
    let mut line = Vec::with_capacity(LONGEST);
    while !line.ends_with(b"\r\n") {
        if line.len() == LONGEST {
            return Err(io::ErrorKind::InvalidData.into());
        }
        line.push(stream.read_u8().await?);
    }
    let line = String::from_utf8_lossy(&line);
    let mut words = line.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("PROXY"), Some("TCP4" | "TCP6"), Some(from)) => {
            from.parse().map_err(|_| io::ErrorKind::InvalidData.into())
        }
        _ => Err(io::ErrorKind::InvalidData.into()),
    }
}

/// The socket a door passes connections to. Each is taken up once its line
/// has been read; one that sends none is dropped.
pub struct Doored(pub UnixListener);

impl Listener for Doored {
    type Io = UnixStream;
    type Addr = Client;

    async fn accept(&mut self) -> (UnixStream, Client) {
        loop {
            let Ok((mut stream, _)) = self.0.accept().await else {
                // Out of file handles, most likely: give the rest a moment.
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            if let Ok(Ok(from)) = timeout(PATIENCE, announced(&mut stream)).await {
                return (stream, Client(from));
            }
        }
    }

    fn local_addr(&self) -> io::Result<Client> {
        Ok(Client(Ipv4Addr::UNSPECIFIED.into()))
    }
}

impl Connected<IncomingStream<'_, Doored>> for Client {
    fn connect_info(stream: IncomingStream<'_, Doored>) -> Self {
        *stream.remote_addr()
    }
}

/// Where Homewarp listens on a port itself, with no door before it.
impl Connected<IncomingStream<'_, TcpListener>> for Client {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Self(stream.remote_addr().ip().to_canonical())
    }
}

/// Who a request came from. Where that is not known, which is in the tests,
/// it is this machine.
impl<S: Send + Sync> FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<ConnectInfo<Self>>()
            .map_or(Self(Ipv4Addr::LOCALHOST.into()), |known| known.0))
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use super::{announce, announced};

    #[tokio::test]
    async fn a_door_says_where_a_connection_came_from_and_core_reads_it() {
        for (from, at, said) in [
            (
                "203.0.113.50:51000",
                "0.0.0.0:3600",
                "PROXY TCP4 203.0.113.50 0.0.0.0 51000 3600\r\n",
            ),
            (
                "[2001:db8::7]:51000",
                "[::]:3600",
                "PROXY TCP6 2001:db8::7 :: 51000 3600\r\n",
            ),
            // An IPv4 address that arrived on a socket for both is itself again.
            (
                "[::ffff:192.168.1.9]:51000",
                "0.0.0.0:3600",
                "PROXY TCP4 192.168.1.9 0.0.0.0 51000 3600\r\n",
            ),
        ] {
            let from: SocketAddr = from.parse().unwrap();
            let line = announce(from, at.parse().unwrap());
            assert_eq!(line, said);
            // What follows the line is left for whoever reads next.
            let mut stream = [line.as_bytes(), b"GET / HTTP/1.1\r\n"].concat();
            let mut reading = stream.as_slice();
            assert_eq!(
                announced(&mut reading).await.unwrap(),
                from.ip().to_canonical()
            );
            assert_eq!(reading, b"GET / HTTP/1.1\r\n");
            stream.clear();
        }
    }

    #[tokio::test]
    async fn what_is_not_a_doors_line_is_not_believed() {
        let long = format!("PROXY TCP4 {}\r\n", "1".repeat(200));
        for odd in [
            "GET / HTTP/1.1\r\n",
            "PROXY UNKNOWN\r\n",
            "PROXY TCP4 not-an-address 0.0.0.0 1 2\r\n",
            "PROXY TCP4 203.0.113.50",
            long.as_str(),
            "",
        ] {
            let mut reading = odd.as_bytes();
            assert!(
                announced(&mut reading).await.is_err(),
                "{odd:?} was read as {:?}",
                None::<IpAddr>
            );
        }
    }
}
