//! SFTP for a server's files (PLAN.md §11, Phase 4): the files the panel's
//! file manager shows, for a program made to move many of them.
//!
//! A sign-in names an account and a server at once: `sam.3` is the account
//! `sam` at the server whose id is 3, with that account's own password. It is
//! let in if the account may do what the Files tab does there. From then on
//! every path is opened through [`ServerDir`], as the file manager's are, so a
//! path or a link that leads out of the server's files leads nowhere.

use std::{
    collections::HashMap,
    io,
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use anyhow::Context;
use homewarp_runtime::{Entry, How, ServerDir, Stat};
use russh::{
    Channel, ChannelId, MethodKind, MethodSet,
    keys::{PrivateKey, ssh_key::private::Ed25519Keypair},
    server::{Auth, ChannelOpenHandle, Config, Handler, Msg, Session},
};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, UnixListener},
    sync::broadcast::error::RecvError,
    time::timeout,
};

use crate::{
    accounts::{self, Permission},
    api::{AppState, Trying, User},
    audit, auth,
    door::{self, Client},
    files,
    runtime::files_at,
    totp,
};

/// How long a wrong sign-in is made to take, whatever was wrong with it.
const REJECTION: Duration = Duration::from_secs(2);
/// How many times one connection may try.
const TRIES: usize = 3;
/// A connection that has been silent this long is closed.
const SILENCE: Duration = Duration::from_secs(600);
/// A door says at once where a connection came from. What has not been said by now will not be.
const PATIENCE: Duration = Duration::from_secs(2);
/// How many files and folders one connection holds open at once.
const MOST_OPEN: usize = 64;
/// The most that is read for one request, whatever the client asks for.
const MOST_READ: usize = 256 << 10;

/// The key this Homewarp shows to whoever connects, so that they know it
/// again. Made the first time, and kept beside the database.
fn host_key(data: &Path) -> anyhow::Result<PrivateKey> {
    let kept = data.join("sftp_host_key");
    let seed: [u8; 32] = match std::fs::read(&kept) {
        Ok(bytes) => bytes
            .try_into()
            .ok()
            .with_context(|| format!("{} is not a host key", kept.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut seed = [0; 32];
            getrandom::fill(&mut seed).context("asking the system for randomness")?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&kept)?;
            io::Write::write_all(&mut file, &seed)?;
            seed
        }
        Err(error) => return Err(error).context("reading the SFTP host key"),
    };
    Ok(PrivateKey::from(Ed25519Keypair::from_seed(&seed)))
}

/// Serves SFTP for as long as Homewarp runs. `listen` is an address, or
/// `unix:<path>` for a socket that a door passes connections to, as the
/// panel's own is (deploy/compose.yml).
pub async fn serve(state: AppState, listen: String) -> anyhow::Result<()> {
    let config = Arc::new(Config {
        // A password and nothing else: it is the account's own, the one the
        // panel asks for.
        methods: MethodSet::from(&[MethodKind::Password][..]),
        auth_rejection_time: REJECTION,
        // A client first asks what it may sign in with. That is no wrong sign-in.
        auth_rejection_time_initial: Some(Duration::ZERO),
        max_auth_attempts: TRIES,
        inactivity_timeout: Some(SILENCE),
        keys: vec![host_key(&state.data)?],
        ..Config::default()
    });
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
            tracing::info!("SFTP is served on {}", socket.display());
            loop {
                let (mut stream, _) = listener.accept().await?;
                let (config, state) = (Arc::clone(&config), state.clone());
                tokio::spawn(async move {
                    // The door says first where the connection came from. One
                    // that says nothing is no door's, and is dropped.
                    let said = timeout(PATIENCE, door::announced(&mut stream)).await;
                    if let Ok(Ok(from)) = said {
                        connection(config, stream, state, Client(from)).await;
                    }
                });
            }
        }
        None => {
            let listener = TcpListener::bind(&listen).await?;
            tracing::info!("SFTP is served on {listen}");
            loop {
                let (stream, from) = listener.accept().await?;
                let from = Client(from.ip().to_canonical());
                tokio::spawn(connection(Arc::clone(&config), stream, state.clone(), from));
            }
        }
    }
}

async fn connection<S>(config: Arc<Config>, stream: S, state: AppState, from: Client)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let holder = Arc::new(Mutex::new(None));
    let mut taken = state.taken.subscribe();
    let ssh = Ssh {
        state: state.clone(),
        from,
        files: None,
        channels: HashMap::new(),
        holder: Arc::clone(&holder),
    };
    let ended = match russh::server::run_stream(config, stream, ssh).await {
        Ok(session) => {
            // What ends the connection from this side. Letting go of the
            // session would not: it goes on by itself.
            let ending = session.handle();
            tokio::pin!(session);
            let mut listening = true;
            loop {
                tokio::select! {
                    ended = &mut session => break ended,
                    // Something was taken from some account. If it was from
                    // this one, the connection is ended: being let in once
                    // is not being let in for good.
                    told = taken.recv(), if listening => {
                        if matches!(told, Err(RecvError::Closed)) {
                            listening = false;
                            continue;
                        }
                        let signed_in = holder.lock().unwrap_or_else(PoisonError::into_inner).clone();
                        if let Some(signed_in) = signed_in
                            && !still(&state, &signed_in).await
                        {
                            let said = "This account may no longer reach these files.".to_owned();
                            let _ = ending
                                .disconnect(russh::Disconnect::ByApplication, said, "en".to_owned())
                                .await;
                            break Ok(());
                        }
                    }
                }
            }
        }
        Err(error) => Err(error),
    };
    // A scanner that hangs up half way is the usual case, and nobody's news.
    if let Err(error) = ended {
        tracing::debug!("an SFTP connection ended badly: {error:#}");
    }
}

/// Who a connection signed in as, to which server's files, and with the
/// password as it was kept then.
#[derive(Clone)]
struct Holder {
    user_id: i64,
    server_id: i64,
    password_hash: String,
}

/// Whether whoever signed in to a connection would still be let in: the
/// account is there, its password is the one it signed in with, and it may
/// still do what the Files tab does with that server.
async fn still(state: &AppState, holder: &Holder) -> bool {
    let found: Result<Option<(String, String, bool)>, sqlx::Error> =
        sqlx::query_as("SELECT username, password_hash, owner FROM users WHERE id = ?")
            .bind(holder.user_id)
            .fetch_optional(&state.db)
            .await;
    match found {
        Ok(Some((username, password_hash, owner))) if password_hash == holder.password_hash => {
            let who = User {
                id: holder.user_id,
                username,
                owner,
            };
            accounts::may(&state.db, &who, holder.server_id, Some(Permission::Files))
                .await
                .is_ok()
        }
        _ => false,
    }
}

/// One connection, as SSH sees it.
struct Ssh {
    state: AppState,
    /// Where it came from.
    from: Client,
    /// The files of the server that was signed in to.
    files: Option<Arc<ServerDir>>,
    /// Channels that are open and have not asked for SFTP yet.
    channels: HashMap<ChannelId, Channel<Msg>>,
    /// Who signed in, once somebody has: for whoever watches the connection
    /// to ask again by, when something is taken from an account.
    holder: Arc<Mutex<Option<Holder>>>,
}

impl Ssh {
    /// The files a name and a password sign in to, or nothing if they sign in
    /// to none.
    async fn sign_in(&self, user: &str, password: &str) -> anyhow::Result<Option<Arc<ServerDir>>> {
        // An account's name may have dots in it. A server's id has none.
        let (username, server) = user.rsplit_once('.').unwrap_or((user, ""));
        // The same count as the panel's sign-in keeps, of the same failures:
        // a password is not guessed here any faster than there.
        let limits = &self.state.limits;
        let trying = Trying::new(self.from, username);
        if trying.may(limits).is_err() {
            return Ok(None);
        }
        let from = self.from.0.to_string();
        let Ok(server_id) = server.parse::<i64>() else {
            trying.failed(limits);
            return Ok(None);
        };
        let db = &self.state.db;
        let found: Option<(i64, String, String, bool)> = sqlx::query_as(
            "SELECT id, username, password_hash, owner FROM users WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(db)
        .await?;
        let Some((id, username, hash, owner)) = found else {
            trying.failed(limits);
            return Ok(None);
        };
        let who = User {
            id,
            username,
            owner,
        };
        // An account with a second step types its code straight after its
        // password, as one word: SFTP has one field to put both in.
        let two_steps: bool =
            sqlx::query_scalar("SELECT totp_secret IS NOT NULL FROM users WHERE id = ?")
                .bind(id)
                .fetch_one(db)
                .await?;
        let (password, code) = match password.len().checked_sub(6) {
            Some(at) if two_steps && password.is_char_boundary(at) => password.split_at(at),
            _ => (password, ""),
        };
        // In its turn, as every password is. One that finds no turn is not let in.
        let right = auth::verify(password.to_owned(), Some(hash.clone()))
            .await
            .unwrap_or(false);
        let right = right && matches!(totp::second_step(db, id, code).await?, totp::Step::Passed);
        if !right {
            trying.failed(limits);
            audit::record(db, &who, Some(server_id), "sftp.sign_in_failed", &from).await;
            return Ok(None);
        }
        trying.passed(limits);
        // What the Files tab asks: to an account that may not, or where there
        // is no such server, there is nothing here to sign in to.
        if accounts::may(db, &who, server_id, Some(Permission::Files))
            .await
            .is_err()
        {
            return Ok(None);
        }
        let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM servers WHERE id = ?")
            .bind(server_id)
            .fetch_optional(db)
            .await?;
        let Some(uuid) = uuid else {
            return Ok(None);
        };
        let path = files_at(&self.state.data, &uuid);
        let (uid, gid) = files::owner();
        let files = tokio::task::spawn_blocking(move || ServerDir::open(&path, uid, gid)).await??;
        audit::record(db, &who, Some(server_id), "sftp.sign_in", &from).await;
        *self.holder.lock().unwrap_or_else(PoisonError::into_inner) = Some(Holder {
            user_id: id,
            server_id,
            password_hash: hash,
        });
        Ok(Some(Arc::new(files)))
    }
}

impl Handler for Ssh {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        match self.sign_in(user, password).await? {
            Some(files) => {
                self.files = Some(files);
                Ok(Auth::Accept)
            }
            None => Ok(Auth::reject()),
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    /// There is no shell here, and no command is run: whoever asks is told so
    /// at once, and not left waiting for an answer.
    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    /// SFTP and nothing else.
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match (name, self.channels.remove(&id), &self.files) {
            ("sftp", Some(channel), Some(files)) => {
                session.channel_success(id)?;
                let sftp = Sftp {
                    files: Arc::clone(files),
                    open: HashMap::new(),
                    opened: 0,
                };
                russh_sftp::server::run(channel.into_stream(), sftp).await;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }
}

/// What a client holds open.
enum Open {
    File(Arc<std::fs::File>),
    /// A folder: what is in it, until the client has been told.
    Folder(Option<Vec<File>>),
}

/// One client's SFTP session in one server's files.
struct Sftp {
    files: Arc<ServerDir>,
    open: HashMap<String, Open>,
    /// How many handles have been given out: the next one's name.
    opened: u64,
}

/// A path as [`ServerDir`] takes it: from the top of the server's files, with
/// `.` and `..` worked out, and never above the top.
fn clean(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// What the file system said, as SFTP says it.
fn code(error: &io::Error) -> StatusCode {
    match error.kind() {
        io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        // What `cap-std` says to a path that leads out of the server's files.
        io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn done(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_owned(),
        language_tag: "en-US".to_owned(),
    }
}

/// A file's or a folder's attributes, as a client lists them.
fn attributes(size: u64, mode: u32, modified: i64) -> FileAttributes {
    let when = u32::try_from(modified).unwrap_or(0);
    let mut attributes = FileAttributes::empty();
    attributes.size = Some(size);
    attributes.permissions = Some(mode);
    attributes.atime = Some(when);
    attributes.mtime = Some(when);
    attributes
}

/// Does something with the server's files off the async threads.
async fn with<T: Send + 'static>(
    files: &Arc<ServerDir>,
    work: impl FnOnce(&ServerDir) -> io::Result<T> + Send + 'static,
) -> Result<T, StatusCode> {
    let files = Arc::clone(files);
    match tokio::task::spawn_blocking(move || work(&files)).await {
        Ok(Ok(made)) => Ok(made),
        Ok(Err(error)) => Err(code(&error)),
        Err(_) => Err(StatusCode::Failure),
    }
}

impl Sftp {
    fn hold(&mut self, id: u32, open: Open) -> Result<Handle, StatusCode> {
        if self.open.len() >= MOST_OPEN {
            return Err(StatusCode::Failure);
        }
        self.opened += 1;
        let handle = self.opened.to_string();
        self.open.insert(handle.clone(), open);
        Ok(Handle { id, handle })
    }

    fn file(&self, handle: &str) -> Result<Arc<std::fs::File>, StatusCode> {
        match self.open.get(handle) {
            Some(Open::File(file)) => Ok(Arc::clone(file)),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn look(&self, id: u32, path: &str, follow: bool) -> Result<Attrs, StatusCode> {
        let path = clean(path);
        let Stat {
            size,
            mode,
            modified,
            ..
        } = with(&self.files, move |files| files.stat(&path, follow)).await?;
        Ok(Attrs {
            id,
            attrs: attributes(size, mode, modified),
        })
    }
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = clean(&filename);
        let how = How {
            read: flags.contains(OpenFlags::READ),
            write: flags.contains(OpenFlags::WRITE),
            append: flags.contains(OpenFlags::APPEND),
            create: flags.contains(OpenFlags::CREATE),
            truncate: flags.contains(OpenFlags::TRUNCATE),
            new: flags.contains(OpenFlags::EXCLUDE),
        };
        let file = with(&self.files, move |files| files.open_with(&path, how)).await?;
        self.hold(id, Open::File(Arc::new(file)))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.open.remove(&handle);
        Ok(done(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let file = self.file(&handle)?;
        let most = (len as usize).min(MOST_READ);
        let read = tokio::task::spawn_blocking(move || {
            let mut data = vec![0; most];
            let read = file.read_at(&mut data, offset)?;
            data.truncate(read);
            io::Result::Ok(data)
        });
        match read.await {
            Ok(Ok(data)) if data.is_empty() => Err(StatusCode::Eof),
            Ok(Ok(data)) => Ok(Data { id, data }),
            Ok(Err(error)) => Err(code(&error)),
            Err(_) => Err(StatusCode::Failure),
        }
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let file = self.file(&handle)?;
        with(&self.files, move |files| {
            // As for an upload through the panel: part of the disk stays free.
            if files.free()? < files::KEPT_FREE {
                return Err(io::ErrorKind::StorageFull.into());
            }
            file.write_all_at(&data, offset)
        })
        .await?;
        Ok(done(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.look(id, &path, false).await
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.look(id, &path, true).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let file = self.file(&handle)?;
        let metadata = file.metadata().map_err(|error| code(&error))?;
        Ok(Attrs {
            id,
            attrs: attributes(metadata.size(), metadata.mode(), metadata.mtime()),
        })
    }

    /// Answered as done, and nothing is done. A client sets a file's mode and
    /// its times after it uploads it, and would call the upload failed if
    /// that were refused; the files are the server's own, and Homewarp is not
    /// let change the mode of what is not its own (deploy/compose.yml).
    async fn setstat(
        &mut self,
        id: u32,
        _: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(done(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        _: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(done(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let path = clean(&path);
        let entries = with(&self.files, move |files| files.list(&path)).await?;
        let listed = entries
            .into_iter()
            .map(|entry| {
                let Entry {
                    name,
                    size,
                    modified,
                    mode,
                    ..
                } = entry;
                File::new(name, attributes(size, mode, modified))
            })
            .collect();
        self.hold(id, Open::Folder(Some(listed)))
    }

    /// All of a folder at once, and then that there is no more.
    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.open.get_mut(&handle) {
            Some(Open::Folder(listed)) => match listed.take() {
                Some(files) if !files.is_empty() => Ok(Name { id, files }),
                _ => Err(StatusCode::Eof),
            },
            _ => Err(StatusCode::Failure),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = clean(&filename);
        with(&self.files, move |files| files.unlink(&path)).await?;
        Ok(done(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = clean(&path);
        with(&self.files, move |files| files.make_dir(&path)).await?;
        Ok(done(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let path = clean(&path);
        with(&self.files, move |files| files.remove_empty_dir(&path)).await?;
        Ok(done(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(format!("/{}", clean(&path)))],
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let (from, to) = (clean(&oldpath), clean(&newpath));
        with(&self.files, move |files| files.rename(&from, &to)).await?;
        Ok(done(id))
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn a_clients_path_is_read_from_the_top_and_never_above_it() {
        for (given, read) in [
            ("/", ""),
            ("", ""),
            (".", ""),
            ("/world/region", "world/region"),
            ("world//region/", "world/region"),
            ("/world/./region/../level.dat", "world/level.dat"),
            ("../../etc/passwd", "etc/passwd"),
            ("/world/../../..", ""),
        ] {
            assert_eq!(clean(given), read, "{given:?}");
        }
    }
}
