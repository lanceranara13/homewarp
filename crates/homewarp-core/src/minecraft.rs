//! Just enough of Minecraft's protocol, as Java Edition speaks it, for two
//! things (PLAN.md §11, Phase 7): asking a running server who is on it, and
//! standing in for one that is asleep, so that a player who joins wakes it.
//!
//! Both are the opening of a connection and nothing after it: the handshake,
//! the question a game's list of servers asks and its answer, and the first
//! packet of a login. None of that is encrypted or compressed, and it has
//! been the same since Minecraft 1.7.

use std::{
    collections::VecDeque,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use serde::Serialize;
use serde_json::json;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
use utoipa::ToSchema;

/// How long a server has to say who is on it. It is asked on the machine it runs on.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);
/// The most a server's answer may be. One with a picture in it is some tens of kilobytes.
const LONGEST_STATUS: usize = 1 << 18;
/// The version a server is asked as. It answers whatever this is; 47 is 1.8,
/// which is what programs that only ask have long said.
const ASKED_AS: i32 = 47;
/// The most a packet may be that a stand-in is sent. A login's first packet
/// is a name and little more.
const LONGEST_OPENING: usize = 1024;
/// How long a stand-in listens to one connection.
const HEARD_WITHIN: Duration = Duration::from_secs(10);
/// How many connections a stand-in listens to at once. One more takes the
/// place of the one that has been listened to longest: a player says who they
/// are in a moment, and is not kept out by connections that say nothing.
const AT_ONCE: usize = 64;
/// How many of the names a server gives are kept.
const MOST_NAMES: usize = 12;
/// What follows a handshake: the list's question, a login, or a login from another server.
const STATUS: i32 = 1;
const LOGIN: i32 = 2;
const TRANSFER: i32 = 3;

/// What a stand-in prints once it listens, and when a player has tried to join.
const LISTENING: &str = "listening";
const JOINED: &str = "joined ";

fn invalid(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// A number as the protocol writes it: seven bits to a byte, the lowest first.
fn put_varint(into: &mut Vec<u8>, value: i32) {
    let mut value = value.cast_unsigned();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            into.push(byte);
            return;
        }
        into.push(byte | 0x80);
    }
}

/// Takes such a number off the front of `from`.
fn take_varint(from: &mut &[u8]) -> Option<i32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let (byte, rest) = from.split_first()?;
        *from = rest;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value.cast_signed());
        }
    }
    None
}

fn put_string(into: &mut Vec<u8>, text: &str) {
    put_varint(into, i32::try_from(text.len()).unwrap_or(i32::MAX));
    into.extend_from_slice(text.as_bytes());
}

/// Takes a text of at most `longest` bytes off the front of `from`.
fn take_string(from: &mut &[u8], longest: usize) -> Option<String> {
    let length = usize::try_from(take_varint(from)?).ok()?;
    if length > longest || length > from.len() {
        return None;
    }
    let (text, rest) = from.split_at(length);
    *from = rest;
    String::from_utf8(text.to_vec()).ok()
}

/// A packet: how long it is, its number, and what it carries.
fn packet(id: i32, carries: &[u8]) -> Vec<u8> {
    let mut inside = Vec::with_capacity(carries.len() + 1);
    put_varint(&mut inside, id);
    inside.extend_from_slice(carries);
    let mut whole = Vec::with_capacity(inside.len() + 3);
    put_varint(&mut whole, i32::try_from(inside.len()).unwrap_or(i32::MAX));
    whole.extend(inside);
    whole
}

/// Reads one packet of at most `longest` bytes: its number, and what it carries.
async fn read_packet<S: AsyncRead + Unpin>(
    from: &mut S,
    longest: usize,
) -> io::Result<(i32, Vec<u8>)> {
    let mut length = 0u32;
    let mut shift = 0;
    loop {
        let byte = from.read_u8().await?;
        length |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 21 {
            return Err(invalid("a packet with no length"));
        }
    }
    let length = length as usize;
    if length == 0 || length > longest {
        return Err(invalid("a packet of a length that is none"));
    }
    let mut inside = vec![0; length];
    from.read_exact(&mut inside).await?;
    let mut rest = inside.as_slice();
    let id = take_varint(&mut rest).ok_or_else(|| invalid("a packet with no number"))?;
    Ok((id, rest.to_vec()))
}

/// Who is on a server, as it says itself to anyone who asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct Players {
    pub(crate) online: u32,
    /// How many it lets on at once.
    pub(crate) max: u32,
    /// Some of their names. A server gives a sample, and a full one not even that.
    pub(crate) names: Vec<String>,
}

/// A name as a server gave it, without the codes that colour it in the game.
fn tidy(name: &str) -> String {
    let mut tidied = String::new();
    let mut letters = name.chars();
    while let Some(letter) = letters.next() {
        match letter {
            '§' => {
                letters.next();
            }
            letter if letter.is_control() => {}
            letter => tidied.push(letter),
        }
    }
    tidied.trim().chars().take(40).collect()
}

/// What a server's answer says of its players. None if it says nothing of them.
fn read(status: &str) -> Option<Players> {
    let status: serde_json::Value = serde_json::from_str(status).ok()?;
    let players = status.get("players")?;
    let count = |name: &str| {
        let count = players.get(name)?.as_i64()?;
        u32::try_from(count.max(0)).ok()
    };
    let names = players
        .get("sample")
        .and_then(|sample| sample.as_array())
        .map(|sample| {
            sample
                .iter()
                .filter_map(|one| one.get("name")?.as_str())
                .map(tidy)
                .filter(|name| !name.is_empty())
                .take(MOST_NAMES)
                .collect()
        })
        .unwrap_or_default();
    Some(Players {
        online: count("online")?,
        max: count("max")?,
        names,
    })
}

/// What the server at `at` answers a game's list of servers, as it is sent.
async fn status(at: SocketAddr) -> io::Result<String> {
    let mut stream = TcpStream::connect(at).await?;
    let mut opening = Vec::new();
    put_varint(&mut opening, ASKED_AS);
    put_string(&mut opening, &at.ip().to_string());
    opening.extend(at.port().to_be_bytes());
    put_varint(&mut opening, STATUS);
    let mut sent = packet(0, &opening);
    sent.extend(packet(0, &[]));
    stream.write_all(&sent).await?;
    let (id, answer) = read_packet(&mut stream, LONGEST_STATUS).await?;
    let mut answer = answer.as_slice();
    (id == 0)
        .then(|| take_string(&mut answer, LONGEST_STATUS))
        .flatten()
        .ok_or_else(|| invalid("an answer that is not a status"))
}

/// Asks the server at `at` who is on it. An error is a server that did not
/// answer as Minecraft does, in time: one of another game, or one not yet up.
pub(crate) async fn players(at: SocketAddr) -> io::Result<Players> {
    let status = timeout(ANSWER_WITHIN, status(at))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no answer in time"))??;
    read(&status).ok_or_else(|| invalid("a status that says nothing of players"))
}

/// What a stand-in tells whoever comes, in place of the server that is asleep.
pub struct StandIn {
    /// Shown in the game's list of servers, where the server's own words would be.
    pub listed: String,
    /// Told to a player who joins.
    pub joining: String,
    /// Whether a player who joins ends it, so that the server is started.
    /// Where not, it stands in until it is taken away.
    pub wakes: bool,
}

/// A player who tried to join a server that is asleep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Joined {
    pub(crate) name: String,
    pub(crate) from: IpAddr,
}

/// Whether this could be a player's name. It is written down and shown, so
/// nothing is taken that is not letters, digits and the few marks names have:
/// the underscore of the game's own, and what a Bedrock player is given in front.
fn is_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|letter| letter.is_ascii_alphanumeric() || b"_.*-".contains(&letter))
}

/// What a stand-in's container runs (`homewarp stand-in <port> <listed>
/// <joining>`): it listens where the server would, answers the list with
/// `listed`, and ends once a player has tried to join, saying who that was.
/// Core, which reads what it prints, then starts the server.
pub async fn stand_in(port: u16, saying: StandIn) -> anyhow::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
    println!("{LISTENING}");
    let joined = stand(listener, Arc::new(saying)).await?;
    println!("{JOINED}{} {}", joined.name, joined.from);
    Ok(())
}

/// The player a stand-in's line names, if it is the line that names one.
pub(crate) fn joined(line: &str) -> Option<Joined> {
    let (name, from) = line.trim().strip_prefix(JOINED)?.split_once(' ')?;
    Some(Joined {
        name: is_name(name).then(|| name.to_owned())?,
        from: from.parse().ok()?,
    })
}

/// Answers on `listener` until a player tries to join, and says who that was.
async fn stand(listener: TcpListener, saying: Arc<StandIn>) -> io::Result<Joined> {
    // Those that are being listened to, the one that came first at the front.
    let mut heard: VecDeque<JoinHandle<()>> = VecDeque::new();
    let (tell, mut told) = mpsc::channel(1);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                // Out of files, say, for a moment. It is not a reason to stop standing in.
                let Ok((stream, from)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                heard.retain(|one| !one.is_finished());
                if heard.len() >= AT_ONCE
                    && let Some(longest) = heard.pop_front()
                {
                    longest.abort();
                }
                let (saying, tell) = (Arc::clone(&saying), tell.clone());
                heard.push_back(tokio::spawn(async move {
                    if let Ok(Ok(Some(name))) = timeout(HEARD_WITHIN, answer(stream, &saying)).await
                        && saying.wakes
                    {
                        let _ = tell.send(Joined { name, from: from.ip() }).await;
                    }
                }));
            }
            Some(joined) = told.recv() => return Ok(joined),
        }
    }
}

/// Answers one connection: the list's question with what the stand-in says of
/// the server, and a login with why it is not let in just now. The name of a
/// player who tried to join is returned.
async fn answer<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    saying: &StandIn,
) -> io::Result<Option<String>> {
    let (id, opening) = read_packet(&mut stream, LONGEST_OPENING).await?;
    let mut opening = opening.as_slice();
    let handshake = (|| {
        let version = take_varint(&mut opening)?;
        // The name and the port the player typed, which are of no use here.
        take_string(&mut opening, 1020)?;
        opening = opening.get(2..)?;
        Some((version, take_varint(&mut opening)?))
    })();
    let Some((version, next)) = handshake.filter(|_| id == 0) else {
        return Ok(None);
    };
    match next {
        STATUS => loop {
            let (id, carried) = read_packet(&mut stream, LONGEST_OPENING).await?;
            match id {
                0 => {
                    // The version the game said it speaks, given back, so that it shows
                    // what is said here and not that the server is of another version.
                    let status = json!({
                        "version": { "name": "Homewarp", "protocol": version },
                        "players": { "max": 0, "online": 0 },
                        "description": { "text": saying.listed },
                    });
                    let mut carries = Vec::new();
                    put_string(&mut carries, &status.to_string());
                    stream.write_all(&packet(0, &carries)).await?;
                }
                // The game times the way there and back with this, and is done.
                1 => {
                    stream.write_all(&packet(1, &carried)).await?;
                    stream.shutdown().await?;
                    return Ok(None);
                }
                _ => return Ok(None),
            }
        },
        LOGIN | TRANSFER => {
            let (id, carried) = read_packet(&mut stream, LONGEST_OPENING).await?;
            let mut carried = carried.as_slice();
            let name = (id == 0).then(|| take_string(&mut carried, 64)).flatten();
            let mut carries = Vec::new();
            put_string(&mut carries, &json!({ "text": saying.joining }).to_string());
            stream.write_all(&packet(0, &carries)).await?;
            stream.shutdown().await?;
            Ok(name.filter(|name| is_name(name)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use super::{
        ASKED_AS, Joined, LOGIN, Players, StandIn, joined, packet, players, put_string, put_varint,
        read, read_packet, stand, status, take_string, take_varint,
    };

    fn varint(value: i32) -> Vec<u8> {
        let mut written = Vec::new();
        put_varint(&mut written, value);
        written
    }

    #[test]
    fn numbers_are_written_as_the_protocol_writes_them() {
        // The examples of the protocol's own description.
        for (value, written) in [
            (0, vec![0x00]),
            (1, vec![0x01]),
            (127, vec![0x7f]),
            (128, vec![0x80, 0x01]),
            (255, vec![0xff, 0x01]),
            (25565, vec![0xdd, 0xc7, 0x01]),
            (2_097_151, vec![0xff, 0xff, 0x7f]),
            (i32::MAX, vec![0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, vec![0xff, 0xff, 0xff, 0xff, 0x0f]),
        ] {
            assert_eq!(varint(value), written, "{value}");
            assert_eq!(take_varint(&mut written.as_slice()), Some(value), "{value}");
        }
        // One that never ends, and one cut short.
        assert_eq!(take_varint(&mut [0xff; 6].as_slice()), None);
        assert_eq!(take_varint(&mut [0x80].as_slice()), None);
        // A text that says it is longer than what there is.
        assert_eq!(take_string(&mut [0x05, b'a', b'b'].as_slice(), 64), None);
        assert_eq!(take_string(&mut [0x02, b'a', b'b'].as_slice(), 1), None);
    }

    #[test]
    fn a_servers_answer_is_read_for_its_players() {
        let paper = r#"{"version":{"name":"Paper 1.21.1","protocol":767},"enforcesSecureChat":true,
            "description":"A Minecraft Server","players":{"max":20,"online":2,"sample":[
            {"name":"Alice","id":"0b0e8a0c-0000-4000-8000-000000000001"},
            {"name":"§a§lBob\n","id":"0b0e8a0c-0000-4000-8000-000000000002"}]}}"#;
        assert_eq!(
            read(paper),
            Some(Players {
                online: 2,
                max: 20,
                names: vec!["Alice".to_owned(), "Bob".to_owned()],
            })
        );
        // Nobody on, and so no names; and a proxy that counts in its own way.
        let empty = r#"{"players":{"max":500,"online":0},"description":{"text":"x"}}"#;
        assert_eq!(
            read(empty).map(|players| (players.online, players.max)),
            Some((0, 500))
        );
        let odd = r#"{"players":{"max":-1,"online":-1}}"#;
        assert_eq!(
            read(odd).map(|players| (players.online, players.max)),
            Some((0, 0))
        );
        // A server that hides its players says nothing that can be counted.
        assert_eq!(read(r#"{"description":"x"}"#), None);
        assert_eq!(read("not json"), None);
    }

    #[test]
    fn a_stand_ins_line_names_who_joined() {
        assert_eq!(
            joined("joined Steve_01 203.0.113.50\r\n"),
            Some(Joined {
                name: "Steve_01".to_owned(),
                from: "203.0.113.50".parse().unwrap(),
            })
        );
        assert_eq!(joined("listening"), None);
        assert_eq!(joined("joined Steve nowhere"), None);
        assert_eq!(joined("joined <b>Steve</b> 203.0.113.50"), None);
    }

    async fn standing() -> (SocketAddr, tokio::task::JoinHandle<std::io::Result<Joined>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = listener.local_addr().unwrap();
        let saying = Arc::new(StandIn {
            listed: "Survival is asleep. Join to wake it.".to_owned(),
            joining: "Survival is waking up. Try again in a minute.".to_owned(),
            wakes: true,
        });
        (at, tokio::spawn(stand(listener, saying)))
    }

    /// The first two packets of a login, as a game sends them, and what comes back.
    async fn join(at: SocketAddr, name: &str) -> Option<String> {
        let mut stream = TcpStream::connect(at).await.unwrap();
        let mut opening = Vec::new();
        put_varint(&mut opening, 767);
        put_string(&mut opening, "play.example.com");
        opening.extend(25565u16.to_be_bytes());
        put_varint(&mut opening, LOGIN);
        let mut start = Vec::new();
        put_string(&mut start, name);
        // The player's id, which a game has sent after the name since 1.20.2.
        start.extend([7u8; 16]);
        let mut sent = packet(0, &opening);
        sent.extend(packet(0, &start));
        stream.write_all(&sent).await.unwrap();
        let (id, carried) = read_packet(&mut stream, 1024).await.ok()?;
        assert_eq!(id, 0);
        take_string(&mut carried.as_slice(), 1024)
    }

    #[tokio::test]
    async fn a_stand_in_answers_the_list_and_ends_when_a_player_joins() {
        let (at, standing) = standing().await;
        // Asked as a game's list asks, by the same code that asks a real server.
        let said: serde_json::Value = serde_json::from_str(&status(at).await.unwrap()).unwrap();
        assert_eq!(
            said["description"]["text"],
            "Survival is asleep. Join to wake it."
        );
        assert_eq!(said["version"]["protocol"], ASKED_AS);
        assert_eq!(
            players(at).await.unwrap(),
            Players {
                online: 0,
                max: 0,
                names: Vec::new()
            }
        );
        // Asked any number of times, it is still standing in.
        assert!(!standing.is_finished());

        let told = join(at, "Steve").await.unwrap();
        let told: serde_json::Value = serde_json::from_str(&told).unwrap();
        assert_eq!(
            told["text"],
            "Survival is waking up. Try again in a minute."
        );
        assert_eq!(
            standing.await.unwrap().unwrap(),
            Joined {
                name: "Steve".to_owned(),
                from: "127.0.0.1".parse().unwrap()
            }
        );
    }

    #[tokio::test]
    async fn connections_that_say_nothing_keep_no_player_from_being_heard() {
        let (at, standing) = standing().await;
        // As many as are listened to at once, and then as many again.
        let mut silent = Vec::new();
        for _ in 0..2 * super::AT_ONCE {
            silent.push(TcpStream::connect(at).await.unwrap());
        }
        // A player is heard all the same, and wakes the server.
        assert!(join(at, "Alex").await.is_some());
        assert_eq!(standing.await.unwrap().unwrap().name, "Alex");
        // The first to have come made way; those that came last are still held.
        let mut nothing = [0u8; 1];
        let first = silent[0].read(&mut nothing).await;
        assert!(matches!(first, Ok(0) | Err(_)), "{first:?}");
    }

    #[tokio::test]
    async fn a_stand_in_for_a_stopped_server_says_so_and_stays() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = listener.local_addr().unwrap();
        let saying = Arc::new(StandIn {
            listed: "Survival is offline.".to_owned(),
            joining: "Survival is offline.".to_owned(),
            wakes: false,
        });
        let standing = tokio::spawn(stand(listener, saying));
        let said: serde_json::Value = serde_json::from_str(&status(at).await.unwrap()).unwrap();
        assert_eq!(said["description"]["text"], "Survival is offline.");
        // However many join, each is told, and none of them ends it.
        for name in ["Steve", "Alex", "Steve"] {
            let told = join(at, name).await.unwrap();
            assert!(told.contains("Survival is offline."), "{told}");
        }
        tokio::task::yield_now().await;
        assert!(!standing.is_finished());
        standing.abort();
    }

    #[tokio::test]
    async fn what_is_not_a_player_joining_wakes_nothing() {
        let (at, standing) = standing().await;
        // Not Minecraft at all, and the old way of asking, from before 1.7.
        for sent in [
            &b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            &[0xfe, 0x01],
            &[0x00],
            &[],
        ] {
            let mut stream = TcpStream::connect(at).await.unwrap();
            stream.write_all(sent).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut back = Vec::new();
            let _ = stream.read_to_end(&mut back).await;
            assert!(back.is_empty(), "{sent:?} was answered with {back:?}");
        }
        // A packet that says it is longer than a login's is not waited for.
        let mut stream = TcpStream::connect(at).await.unwrap();
        stream.write_all(&[0xff, 0xff, 0x03]).await.unwrap();
        let mut back = Vec::new();
        let _ = stream.read_to_end(&mut back).await;
        assert!(back.is_empty());
        // A name that is none is told what a player is told, and wakes nothing.
        for name in ["", "two words", "<script>", "a\nb", &"x".repeat(33)] {
            assert!(
                join(at, name)
                    .await
                    .is_some_and(|told| told.contains("waking up")),
                "{name:?}"
            );
        }
        tokio::task::yield_now().await;
        assert!(!standing.is_finished());
        // And after all of that a player still does.
        join(at, ".Bedrock_Alex").await.unwrap();
        assert_eq!(standing.await.unwrap().unwrap().name, ".Bedrock_Alex");
    }
}
