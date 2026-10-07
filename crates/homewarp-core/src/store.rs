//! A store elsewhere for backups (PLAN.md §11, Phase 7): a bucket that is
//! spoken to as Amazon's S3 is, which is how MinIO, Cloudflare's R2,
//! Backblaze's B2 and many others are spoken to as well.
//!
//! A backup beside the server it is of is lost with the disk they are both on.
//! With a store set, each backup that is made is copied there too, and the
//! copy goes when the backup goes.
//!
//! Three requests are all this speaks: put a file, delete one, and (to try a
//! store before trusting it) put and delete a few bytes. Each is signed the
//! way S3 has requests signed (Signature Version 4), with the key the owner
//! gave. The store is the owner's, at an address the owner typed, and may be
//! on the home network or on plain HTTP: it is not held to what a fetch from
//! a stranger's address is held to (`fetch`), because it is not one.

use std::{io, net::SocketAddr, time::Duration};

use axum::http::{
    Method, Request, StatusCode,
    header::{CONTENT_LENGTH, HOST},
};
use futures_util::stream;
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::body::{Bytes, Frame};
use ring::{digest, hmac};
use rustls::pki_types::ServerName;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::{io::AsyncReadExt, net::TcpStream, time::timeout};
use tokio_rustls::TlsConnector;

use crate::{auth, clock, fetch};

/// The key in `settings`.
const STORE: &str = "backup_store";
/// How long a store is given to take a connection.
const CONNECTING: Duration = Duration::from_secs(8);
/// How long a small request may take.
const ASKING: Duration = Duration::from_secs(30);
/// The slowest a file may go up before it is given up on: about 400 kbit/s.
const SLOWEST: u64 = 50_000;
/// The most that S3 takes in one request. A larger file would go up in parts.
const LARGEST: u64 = 5 << 30;
/// How much of a file is read at a time.
const CHUNK: usize = 256 << 10;
/// How much of a refusal is read. It is a few lines of XML.
const REFUSAL: usize = 8 << 10;
/// The checksum of nothing, and what is said in place of a checksum for a
/// file that is not read twice. S3 takes the second over TLS, and so do the
/// stores that speak its language.
const NOTHING: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

/// A sentence for whoever asked, about why the store did not do it.
pub(crate) type Refused = String;

/// Where backups are copied to, and the key that lets Homewarp write there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Store {
    /// `https://` or `http://`, a host, and a port if it is not the usual one.
    pub(crate) endpoint: String,
    pub(crate) region: String,
    pub(crate) bucket: String,
    pub(crate) key_id: String,
    pub(crate) secret: String,
    /// A folder in the bucket, so to speak. Empty for none.
    pub(crate) prefix: String,
}

/// Where a store is reached.
#[derive(Debug, PartialEq, Eq)]
struct Place {
    tls: bool,
    host: String,
    port: u16,
}

impl Place {
    /// The host as a request names it: with its port, unless that is the usual one.
    fn named(&self) -> String {
        match (self.tls, self.port) {
            (true, 443) | (false, 80) => self.host.clone(),
            (_, port) => format!("{}:{port}", self.host),
        }
    }
}

/// Reads a store's address.
fn place(endpoint: &str) -> Result<Place, Refused> {
    let refused = || {
        Err(
            "A store's address is like https://s3.example.com or http://192.168.1.20:9000: no folder after it, and no name or password in it."
                .to_owned(),
        )
    };
    let endpoint = endpoint.trim().trim_end_matches('/');
    let (tls, rest) = match (
        endpoint.strip_prefix("https://"),
        endpoint.strip_prefix("http://"),
    ) {
        (Some(rest), _) => (true, rest),
        (_, Some(rest)) => (false, rest),
        _ => return refused(),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) if port != 0 => (host, port),
            _ => return refused(),
        },
        None => (rest, if tls { 443 } else { 80 }),
    };
    let plain = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && host
            .bytes()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, b'.' | b'-'));
    if !plain {
        return refused();
    }
    Ok(Place {
        tls,
        host: host.to_lowercase(),
        port,
    })
}

impl Store {
    /// Holds what the owner typed against what a store's settings look like,
    /// and returns them tidied. Nothing is asked of the store.
    pub(crate) fn checked(self) -> Result<Self, Refused> {
        let endpoint = self.endpoint.trim().trim_end_matches('/').to_owned();
        place(&endpoint)?;
        let bucket = self.bucket.trim().to_owned();
        let named = (3..=63).contains(&bucket.len())
            && bucket.bytes().all(|letter| {
                letter.is_ascii_lowercase()
                    || letter.is_ascii_digit()
                    || matches!(letter, b'.' | b'-')
            })
            && bucket.starts_with(|first: char| first.is_ascii_alphanumeric())
            && bucket.ends_with(|last: char| last.is_ascii_alphanumeric());
        if !named {
            return Err(
                "A bucket's name is 3 to 63 small letters, digits, dots and hyphens.".to_owned(),
            );
        }
        let region = match self.region.trim() {
            "" => "us-east-1".to_owned(),
            region => region.to_owned(),
        };
        if region.len() > 40
            || !region
                .bytes()
                .all(|letter| letter.is_ascii_alphanumeric() || letter == b'-')
        {
            return Err("A region is written like us-east-1, or auto.".to_owned());
        }
        let prefix = self.prefix.trim().trim_matches('/').to_owned();
        let foldered = prefix.len() <= 100
            && !prefix.contains("..")
            && !prefix.contains("//")
            && prefix
                .bytes()
                .all(|letter| letter.is_ascii_alphanumeric() || b"._-/".contains(&letter));
        if !foldered {
            return Err(
                "A folder in the bucket is letters, digits, dots, hyphens and slashes, 100 at the most."
                    .to_owned(),
            );
        }
        let key_id = self.key_id.trim().to_owned();
        let secret = self.secret.trim().to_owned();
        let keyed = |key: &str, longest: usize| {
            (1..=longest).contains(&key.len())
                && key.bytes().all(|letter| letter.is_ascii_graphic())
        };
        if !keyed(&key_id, 128) || !keyed(&secret, 256) {
            return Err(
                "Give the key's id and its secret, as the store gave them to you.".to_owned(),
            );
        }
        Ok(Self {
            endpoint,
            region,
            bucket,
            key_id,
            secret,
            prefix,
        })
    }

    /// Where in the bucket something by this name goes.
    pub(crate) fn key(&self, name: &str) -> String {
        match self.prefix.as_str() {
            "" => name.to_owned(),
            prefix => format!("{prefix}/{name}"),
        }
    }
}

/// The store that is set, if one is.
pub(crate) async fn kept(db: &SqlitePool) -> Option<Store> {
    let json: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(STORE)
        .fetch_optional(db)
        .await
        .ok()?;
    serde_json::from_str(&json?).ok()
}

pub(crate) async fn keep(db: &SqlitePool, store: &Store) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(STORE)
    .bind(serde_json::to_string(store)?)
    .execute(db)
    .await?;
    Ok(())
}

pub(crate) async fn forget(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(STORE)
        .execute(db)
        .await?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

fn keyed(key: &[u8], text: &str) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), text.as_bytes())
        .as_ref()
        .to_vec()
}

/// A path as it is signed and sent: each part of it with everything written by
/// its number that is not a letter, a digit or one of four marks.
fn encoded(path: &str) -> String {
    let mut written = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                written.push(char::from(byte));
            }
            other => written.push_str(&format!("%{other:02X}")),
        }
    }
    written
}

/// A moment as S3 has it written: the day, and the day with the time.
fn written(now: i64) -> (String, String) {
    let (year, month, day) = clock::civil(now.div_euclid(86_400));
    let seconds = now.rem_euclid(86_400);
    let date = format!("{year:04}{month:02}{day:02}");
    let time = format!(
        "{date}T{:02}{:02}{:02}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    (date, time)
}

/// What a request is signed with: who signs, for which region, and when.
struct Signer<'a> {
    key_id: &'a str,
    secret: &'a str,
    region: &'a str,
    now: i64,
}

impl Signer<'_> {
    /// The `Authorization` header for a request, by Signature Version 4.
    /// `path` is as it is sent, `query` as it is sent and in order, and
    /// `headers` are those that are signed: small letters, in order of name.
    fn authorization(
        &self,
        method: &str,
        path: &str,
        query: &str,
        headers: &[(&str, &str)],
        payload: &str,
    ) -> String {
        let (date, time) = written(self.now);
        let named: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
        let named = named.join(";");
        let mut request = format!("{method}\n{path}\n{query}\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}:{}\n", value.trim()));
        }
        request.push_str(&format!("\n{named}\n{payload}"));
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{time}\n{scope}\n{}",
            sha256(request.as_bytes())
        );
        let key = keyed(format!("AWS4{}", self.secret).as_bytes(), &date);
        let key = keyed(&key, self.region);
        let key = keyed(&key, "s3");
        let key = keyed(&key, "aws4_request");
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={named},Signature={}",
            self.key_id,
            hex(&keyed(&key, &to_sign))
        )
    }
}

type Sent = UnsyncBoxBody<Bytes, io::Error>;

fn nothing() -> Sent {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn these(bytes: Bytes) -> Sent {
    Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// A file as the body of a request, read as it is sent and not before.
fn file(file: tokio::fs::File, length: u64) -> Sent {
    let chunks = stream::try_unfold((file, length), |(mut file, left)| async move {
        if left == 0 {
            return Ok(None);
        }
        let mut chunk = vec![0; CHUNK.min(usize::try_from(left).unwrap_or(CHUNK))];
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the file ended before all of it was sent",
            ));
        }
        chunk.truncate(read);
        Ok(Some((
            Frame::data(Bytes::from(chunk)),
            (file, left - read as u64),
        )))
    });
    StreamBody::new(chunks).boxed_unsync()
}

/// What a store said when it refused: its code and its sentence, out of the
/// XML it answers with.
fn refusal(status: StatusCode, sent: Option<&[u8]>) -> Refused {
    let text = sent.map(String::from_utf8_lossy).unwrap_or_default();
    let between = |open: &str, close: &str| {
        let after = text.split_once(open)?.1;
        Some(
            after
                .split_once(close)?
                .0
                .trim()
                .chars()
                .take(200)
                .collect::<String>(),
        )
    };
    match (
        between("<Code>", "</Code>"),
        between("<Message>", "</Message>"),
    ) {
        (Some(code), Some(message)) => format!("The store refused ({status}, {code}): {message}"),
        (Some(code), None) => format!("The store refused ({status}, {code})."),
        _ => format!("The store refused ({status})."),
    }
}

impl Store {
    /// One signed request, and whether the store did what was asked.
    async fn ask(
        &self,
        method: Method,
        key: &str,
        payload: &str,
        length: u64,
        body: Sent,
    ) -> Result<(), Refused> {
        let place = place(&self.endpoint)?;
        let named = place.named();
        let path = encoded(&format!("/{}/{key}", self.bucket));
        let now = auth::now();
        let (_, time) = written(now);
        let signer = Signer {
            key_id: &self.key_id,
            secret: &self.secret,
            region: &self.region,
            now,
        };
        let authorization = signer.authorization(
            method.as_str(),
            &path,
            "",
            &[
                ("host", &named),
                ("x-amz-content-sha256", payload),
                ("x-amz-date", &time),
            ],
            payload,
        );
        let request = Request::builder()
            .method(method)
            .uri(&path)
            .header(HOST, &named)
            .header("x-amz-content-sha256", payload)
            .header("x-amz-date", &time)
            .header("authorization", authorization)
            .header(CONTENT_LENGTH, length)
            .body(body)
            .map_err(|error| format!("The store cannot be asked that: {error}."))?;

        let host = place.host.as_str();
        let found: Vec<SocketAddr> = tokio::net::lookup_host((host, place.port))
            .await
            .map_err(|_| format!("{host} could not be looked up."))?
            .collect();
        let mut stream = None;
        for address in found {
            if let Ok(Ok(connected)) = timeout(CONNECTING, TcpStream::connect(address)).await {
                stream = Some(connected);
                break;
            }
        }
        let stream = stream.ok_or_else(|| format!("{named} could not be reached."))?;
        let said = if place.tls {
            let name = ServerName::try_from(host.to_owned())
                .map_err(|_| format!("{host} is not a name a certificate can be for."))?;
            let stream = TlsConnector::from(fetch::trusting()?)
                .connect(name, stream)
                .await
                .map_err(|error| {
                    format!("{host} did not show a certificate this machine trusts: {error}.")
                })?;
            fetch::exchange(stream, host, request, REFUSAL).await?
        } else {
            fetch::exchange(stream, host, request, REFUSAL).await?
        };
        match said.status {
            status if status.is_success() => Ok(()),
            status => Err(refusal(status, said.sent.as_deref())),
        }
    }

    /// Copies a file to the store under `key`. It is read as it is sent, and
    /// given as long as a slow line would need for it.
    pub(crate) async fn put(&self, key: &str, from: &std::path::Path) -> Result<(), Refused> {
        let opened = tokio::fs::File::open(from)
            .await
            .map_err(|error| format!("The file could not be opened: {error}."))?;
        let length = opened
            .metadata()
            .await
            .map_err(|error| format!("The file could not be read: {error}."))?
            .len();
        if length > LARGEST {
            return Err(format!(
                "It is {} GB, and 5 GB is the most a store takes in one piece.",
                length >> 30
            ));
        }
        let patience = ASKING + Duration::from_secs(length / SLOWEST);
        let sent = self.ask(Method::PUT, key, UNSIGNED, length, file(opened, length));
        match timeout(patience, sent).await {
            Ok(sent) => sent,
            Err(_) => Err("The store took too long to take it.".to_owned()),
        }
    }

    /// Removes what is under `key`. A store says it has done so whether or
    /// not anything was there.
    pub(crate) async fn remove(&self, key: &str) -> Result<(), Refused> {
        let asked = self.ask(Method::DELETE, key, NOTHING, 0, nothing());
        match timeout(ASKING, asked).await {
            Ok(asked) => asked,
            Err(_) => Err("The store took too long to answer.".to_owned()),
        }
    }

    /// Tries the store: writes a few bytes under a name of Homewarp's own and
    /// takes them away again, which is everything a backup will need of it.
    pub(crate) async fn tried(&self) -> Result<(), Refused> {
        let key = self.key("homewarp-was-here");
        let words = Bytes::from_static(b"Homewarp can write to this bucket.\n");
        let tried = async {
            let length = words.len() as u64;
            self.ask(Method::PUT, &key, &sha256(&words), length, these(words))
                .await?;
            self.ask(Method::DELETE, &key, NOTHING, 0, nothing()).await
        };
        match timeout(ASKING, tried).await {
            Ok(tried) => tried,
            Err(_) => Err("The store took too long to answer.".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::{NOTHING, Place, Signer, Store, encoded, place, refusal, sha256, written};

    /// The key Amazon signs its own examples with, and the moment it signs them at.
    fn amazons() -> Signer<'static> {
        Signer {
            key_id: "AKIAIOSFODNN7EXAMPLE",
            secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            // 2013-05-24T00:00:00Z
            now: 1_369_353_600,
        }
    }

    #[test]
    fn a_moment_is_written_as_s3_has_it() {
        assert_eq!(
            written(1_369_353_600),
            ("20130524".to_owned(), "20130524T000000Z".to_owned())
        );
        assert_eq!(
            written(1_791_333_715),
            ("20261007".to_owned(), "20261007T004155Z".to_owned())
        );
    }

    #[test]
    fn requests_are_signed_as_amazons_own_examples_are() {
        // "Signature Calculations for the Authorization Header: Transferring
        // Payload in a Single Chunk", in the S3 API Reference: GET Object.
        let get = amazons().authorization(
            "GET",
            "/test.txt",
            "",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", NOTHING),
                ("x-amz-date", "20130524T000000Z"),
            ],
            NOTHING,
        );
        assert_eq!(
            get,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        // And PUT Object, whose name has a mark in it that is written by its number.
        let welcome = sha256(b"Welcome to Amazon S3.");
        assert_eq!(
            welcome,
            "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072"
        );
        let put = amazons().authorization(
            "PUT",
            &encoded("/test$file.text"),
            "",
            &[
                ("date", "Fri, 24 May 2013 00:00:00 GMT"),
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", &welcome),
                ("x-amz-date", "20130524T000000Z"),
                ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ],
            &welcome,
        );
        assert!(
            put.ends_with(
                "Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
            ),
            "{put}"
        );
        // And a request with a question in it: GET Bucket Lifecycle.
        let lifecycle = amazons().authorization(
            "GET",
            "/",
            "lifecycle=",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("x-amz-content-sha256", NOTHING),
                ("x-amz-date", "20130524T000000Z"),
            ],
            NOTHING,
        );
        assert!(
            lifecycle.ends_with(
                "Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
            ),
            "{lifecycle}"
        );
    }

    #[test]
    fn a_stores_address_is_a_scheme_a_host_and_perhaps_a_port() {
        let at = |tls, host: &str, port| Place {
            tls,
            host: host.to_owned(),
            port,
        };
        assert_eq!(
            place("https://S3.Example.com/"),
            Ok(at(true, "s3.example.com", 443))
        );
        assert_eq!(
            place(" http://192.168.1.20:9000 "),
            Ok(at(false, "192.168.1.20", 9000))
        );
        assert_eq!(place("http://minio"), Ok(at(false, "minio", 80)));
        assert_eq!(at(true, "s3.example.com", 443).named(), "s3.example.com");
        assert_eq!(at(false, "192.168.1.20", 9000).named(), "192.168.1.20:9000");
        assert_eq!(
            at(true, "s3.example.com", 8443).named(),
            "s3.example.com:8443"
        );
        for odd in [
            "s3.example.com",
            "ftp://s3.example.com",
            "https://user:secret@s3.example.com",
            "https://s3.example.com/bucket",
            "https://s3.example.com:0",
            "https://s3.example.com:port",
            "https://",
            "https://exa mple.com",
            "https://example.com?x=1",
            "",
        ] {
            assert!(place(odd).is_err(), "{odd}");
        }
    }

    fn typed() -> Store {
        Store {
            endpoint: " https://s3.example.com/ ".to_owned(),
            region: String::new(),
            bucket: "my-backups".to_owned(),
            key_id: " AKIAEXAMPLE ".to_owned(),
            secret: "s3cr3t/Key+".to_owned(),
            prefix: "/homewarp/home/".to_owned(),
        }
    }

    #[test]
    fn what_was_typed_for_a_store_is_tidied_or_refused() {
        let store = typed().checked().unwrap();
        assert_eq!(store.endpoint, "https://s3.example.com");
        assert_eq!(store.region, "us-east-1");
        assert_eq!(store.key_id, "AKIAEXAMPLE");
        assert_eq!(store.prefix, "homewarp/home");
        assert_eq!(
            store.key("Lobby/3.tar.zst"),
            "homewarp/home/Lobby/3.tar.zst"
        );
        let bare = Store {
            prefix: String::new(),
            ..typed()
        };
        assert_eq!(bare.checked().unwrap().key("a.tar.zst"), "a.tar.zst");

        let wrong = |change: fn(&mut Store)| {
            let mut store = typed();
            change(&mut store);
            store.checked().unwrap_err()
        };
        assert!(wrong(|store| store.bucket = "My_Bucket".to_owned()).contains("bucket's name"));
        assert!(wrong(|store| store.bucket = "ab".to_owned()).contains("bucket's name"));
        assert!(wrong(|store| store.bucket = "a/../b".to_owned()).contains("bucket's name"));
        assert!(wrong(|store| store.region = "us east".to_owned()).contains("region"));
        assert!(wrong(|store| store.prefix = "a/../b".to_owned()).contains("folder in the bucket"));
        assert!(wrong(|store| store.prefix = "a b".to_owned()).contains("folder in the bucket"));
        assert!(wrong(|store| store.secret = String::new()).contains("key's id and its secret"));
        assert!(
            wrong(|store| store.key_id = "a key".to_owned()).contains("key's id and its secret")
        );
        assert!(
            wrong(|store| store.endpoint = "s3.example.com".to_owned()).contains("store's address")
        );
    }

    #[test]
    fn a_path_is_written_for_signing_and_a_refusal_is_read() {
        assert_eq!(
            encoded("/bucket/home/Lobby 1/3.tar.zst"),
            "/bucket/home/Lobby%201/3.tar.zst"
        );
        assert_eq!(encoded("/b/a+b=c$d"), "/b/a%2Bb%3Dc%24d");
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>AccessDenied</Code><Message>Access Denied.</Message><RequestId>1</RequestId></Error>"#;
        assert_eq!(
            refusal(StatusCode::FORBIDDEN, Some(xml)),
            "The store refused (403 Forbidden, AccessDenied): Access Denied."
        );
        assert_eq!(
            refusal(StatusCode::BAD_GATEWAY, None),
            "The store refused (502 Bad Gateway)."
        );
        assert_eq!(
            refusal(
                StatusCode::NOT_FOUND,
                Some(b"<Error><Code>NoSuchBucket</Code></Error>")
            ),
            "The store refused (404 Not Found, NoSuchBucket)."
        );
    }
}
