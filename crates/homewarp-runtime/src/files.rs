use std::{
    borrow::Cow,
    collections::HashSet,
    fs::Permissions,
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsFd,
        unix::fs::{PermissionsExt, fchown},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt},
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use tar::{EntryType, Header};

/// A server's directory, opened so that nothing inside it, symbolic links
/// included, can lead a read or a write outside it.
///
/// A game server, its plugins and its install script all write here, and none of
/// them is trusted: a planted `server.properties -> /etc/shadow` must not turn a
/// config patch into a write to the host. Nor is what a person uploads: an
/// archive names its own files, and may name them `../../etc/cron.d/x`.
pub struct ServerDir {
    dir: Dir,
    uid: u32,
    gid: u32,
}

/// What a name in a folder stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
    /// A symbolic link. Where it leads is not gone to, to find out.
    Link,
    /// A pipe, a socket, a device: nothing a file manager opens.
    Other,
}

impl Kind {
    fn of(metadata: &Metadata) -> Self {
        let kind = metadata.file_type();
        if kind.is_dir() {
            Self::Folder
        } else if kind.is_file() {
            Self::File
        } else if kind.is_symlink() {
            Self::Link
        } else {
            Self::Other
        }
    }
}

/// One thing in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// In bytes. Nothing for what is not a file.
    pub size: u64,
    /// When it was last changed, in Unix seconds.
    pub modified: i64,
    /// Its mode as the file system has it: what it is, and who may do what with it.
    pub mode: u32,
}

/// What is at a path, as a program that works with files asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
    pub modified: i64,
    pub mode: u32,
}

/// How a file is to be opened by a program that will hold it open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct How {
    pub read: bool,
    pub write: bool,
    /// Every write goes to the end.
    pub append: bool,
    /// Made if it is not there.
    pub create: bool,
    /// Emptied if it is.
    pub truncate: bool,
    /// Made, and refused if it is there already.
    pub new: bool,
}

/// What unpacking an archive came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unpacked {
    /// How many files were written.
    pub files: u64,
    /// How many entries were passed over: those that led out of the server's
    /// files, and those that are neither a file, a folder nor a link.
    pub skipped: u64,
}

/// The user and the group this process runs as.
pub fn running_as() -> (u32, u32) {
    (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    )
}

/// Told apart the files that are on their way in at the same moment.
static PARTS: AtomicU64 = AtomicU64::new(0);

const NO_PARENT: &str = "";

/// The most a file of settings may be for Homewarp to read it whole
/// ([`ServerDir::read_to_string`]). A `server.properties` is a few kilobytes,
/// and the longest of a proxy's or a plugin's some hundreds.
pub const LARGEST_SETTINGS: u64 = 4 << 20;

/// How many names are tried for a file on its way in before it is given up on.
const PART_TRIES: u32 = 16;

/// Asked of every open of something a server may have put there: not to wait.
/// To an ordinary file it makes no difference. What is not one is then opened
/// at once, found out for what it is, and refused, where it would otherwise
/// have been waited on for as long as nobody wrote to it.
const NO_WAITING: i32 = rustix::fs::OFlags::NONBLOCK.bits() as i32;

/// How a file is opened to be read.
fn reading() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(NO_WAITING);
    options
}

/// How hard a backup is packed: Zstandard's own middle, which a home machine
/// writes at a few hundred megabytes a second.
const BACKUP_LEVEL: i32 = 3;

/// Where a path that was given from outside leads. An empty one is the
/// directory itself. Links are for `cap-std` to hold in; what is refused here
/// is a path that says outright that it means to climb.
fn at(path: &str) -> io::Result<&Path> {
    let path = Path::new(path.trim_matches('/'));
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    })
}

/// As [`at`], for what has to be something in the directory and not the
/// directory itself.
fn inside(path: &str) -> io::Result<&Path> {
    match at(path)? {
        top if top == Path::new(".") => Err(io::ErrorKind::InvalidInput.into()),
        path => Ok(path),
    }
}

/// How long an ordinary file is, and an error for anything else.
fn ordinary(metadata: &Metadata) -> io::Result<u64> {
    match Kind::of(metadata) {
        Kind::File => Ok(metadata.len()),
        Kind::Folder => Err(io::ErrorKind::IsADirectory.into()),
        Kind::Link | Kind::Other => Err(io::ErrorKind::InvalidInput.into()),
    }
}

impl ServerDir {
    /// Opens the directory, creating it if need be. Files written through it
    /// are given to `uid:gid`, the user the server runs as.
    pub fn open(path: &Path, uid: u32, gid: u32) -> io::Result<Self> {
        std::fs::create_dir_all(path)?;
        Ok(Self {
            dir: Dir::open_ambient_dir(path, ambient_authority())?,
            uid,
            gid,
        })
    }

    /// The text of a file of settings, or `None` if there is no such file.
    ///
    /// It is the server's to write, and so held to what such a file is: an
    /// ordinary file of at most [`LARGEST_SETTINGS`] bytes of text. The error
    /// is `FileTooLarge`, `InvalidData` or `InvalidInput` for what is not.
    pub fn read_to_string(&self, path: &str) -> io::Result<Option<String>> {
        match self.text(path, LARGEST_SETTINGS) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Writes a file, making the directories it sits in if the server has not
    /// made them yet. Those are given to the server's user as the file is.
    ///
    /// What is there by that name is never opened: the text is written
    /// beside it and put in its place. And it is put in the place of an
    /// ordinary file only; the error is `InvalidInput` for a name that is a
    /// link, a pipe or anything else a server may have left there.
    pub fn write(&self, path: &str, contents: &str) -> io::Result<()> {
        if let Ok(there) = self.dir.symlink_metadata(inside(path)?) {
            ordinary(&there)?;
        }
        let (mut file, part) = self.begin(path)?;
        match file.write_all(contents.as_bytes()) {
            Ok(()) => self.finish(&part, path),
            Err(error) => {
                self.abandon(&part);
                Err(error)
            }
        }
    }

    /// What is in a folder: folders first, then by name.
    pub fn list(&self, path: &str) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in self.dir.read_dir(at(path)?)? {
            let entry = entry?;
            // Gone between being listed and being asked about.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let kind = Kind::of(&metadata);
            entries.push(Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
                size: if kind == Kind::File {
                    metadata.len()
                } else {
                    0
                },
                modified: metadata.mtime(),
                mode: metadata.mode(),
            });
        }
        entries.sort_by_cached_key(|entry| (entry.kind != Kind::Folder, entry.name.to_lowercase()));
        Ok(entries)
    }

    /// What is at a path: the directory itself, by the empty path. With
    /// `follow`, what a link there leads to; without, the link.
    pub fn stat(&self, path: &str, follow: bool) -> io::Result<Stat> {
        let path = at(path)?;
        let metadata = match follow {
            true => self.dir.metadata(path)?,
            false => self.dir.symlink_metadata(path)?,
        };
        Ok(Stat {
            kind: Kind::of(&metadata),
            size: metadata.len(),
            modified: metadata.mtime(),
            mode: metadata.mode(),
        })
    }

    /// Opens an ordinary file for a program that will hold it open and read
    /// or write anywhere in it: an SFTP client. A file this makes is given to
    /// the server's user. The folder it is in has to be there.
    pub fn open_with(&self, path: &str, how: How) -> io::Result<std::fs::File> {
        let path = inside(path)?;
        // A pipe is refused before it is opened, as in [`ServerDir::file`].
        let there = match self.dir.metadata(path) {
            Ok(metadata) => {
                ordinary(&metadata)?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        let mut options = OpenOptions::new();
        options
            .read(how.read)
            .write(how.write)
            .append(how.append)
            .create(how.create)
            .truncate(how.truncate)
            .create_new(how.new)
            // What it was a moment ago it may no longer be.
            .custom_flags(NO_WAITING);
        let file = self.dir.open_with(path, &options)?;
        ordinary(&file.metadata()?)?;
        if !there {
            self.own(&file)?;
        }
        Ok(file.into_std())
    }

    /// Deletes one file, or one link. Not a folder.
    pub fn unlink(&self, path: &str) -> io::Result<()> {
        self.dir.remove_file(inside(path)?)
    }

    /// Deletes a folder that has nothing in it.
    pub fn remove_empty_dir(&self, path: &str) -> io::Result<()> {
        self.dir.remove_dir(inside(path)?)
    }

    /// Opens an ordinary file to be read, and says how long it is. What it is
    /// is asked before it is opened as well as after: a server can leave a pipe
    /// here, and opening a pipe waits for someone to write to it.
    pub fn file(&self, path: &str) -> io::Result<(std::fs::File, u64)> {
        let path = inside(path)?;
        ordinary(&self.dir.metadata(path)?)?;
        let file = self.dir.open_with(path, &reading())?;
        let size = ordinary(&file.metadata()?)?;
        Ok((file.into_std(), size))
    }

    /// A file's text. The error is `FileTooLarge` for one longer than `at_most`
    /// bytes, and `InvalidData` for one that is not text.
    pub fn text(&self, path: &str, at_most: u64) -> io::Result<String> {
        let (file, size) = self.file(path)?;
        if size > at_most {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let mut bytes = Vec::with_capacity(size as usize);
        // Not trusted to be as long as it was a moment ago.
        file.take(at_most + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > at_most {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        String::from_utf8(bytes).map_err(|_| io::ErrorKind::InvalidData.into())
    }

    /// Starts a file that will stand at `path` once it is whole, so that an
    /// upload cut off half way leaves what was there as it was. Returns the
    /// file to write to, and the name it goes by until it is put in its place
    /// with [`ServerDir::finish`] or dropped with [`ServerDir::abandon`].
    pub fn begin(&self, path: &str) -> io::Result<(std::fs::File, String)> {
        let path = inside(path)?;
        if self
            .dir
            .symlink_metadata(path)
            .is_ok_and(|there| there.is_dir())
        {
            return Err(io::ErrorKind::IsADirectory.into());
        }
        let parent = path.parent().unwrap_or(Path::new(NO_PARENT));
        self.make_dirs(parent)?;
        // Made, and never opened: a name that something already has is not
        // this file's, whatever a server put there, and the next one is tried.
        let mut making = OpenOptions::new();
        making.write(true).create_new(true);
        let mut tries = 0;
        let (file, part) = loop {
            let part = parent.join(format!(
                ".homewarp-{}-{}.part",
                std::process::id(),
                PARTS.fetch_add(1, Ordering::Relaxed)
            ));
            tries += 1;
            match self.dir.open_with(&part, &making) {
                Ok(file) => break (file.into_std(), part),
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists && tries < PART_TRIES => {}
                Err(error) => return Err(error),
            }
        };
        // A start script that is edited has to stay one that can be run. Set
        // before the file is given away: after, it is no longer Homewarp's to set.
        if let Ok(replaced) = self.dir.metadata(path)
            && replaced.is_file()
        {
            file.set_permissions(Permissions::from_mode(replaced.mode() & 0o777))?;
        }
        self.own(&file)?;
        Ok((file, part.to_string_lossy().into_owned()))
    }

    /// Puts a file begun with [`ServerDir::begin`] in its place.
    pub fn finish(&self, part: &str, path: &str) -> io::Result<()> {
        self.dir.rename(part, &self.dir, inside(path)?)
    }

    /// Drops a file begun with [`ServerDir::begin`].
    pub fn abandon(&self, part: &str) {
        let _ = self.dir.remove_file(part);
    }

    /// Makes a folder, and those it sits in if they are not there.
    pub fn make_dir(&self, path: &str) -> io::Result<()> {
        let path = inside(path)?;
        if self.dir.symlink_metadata(path).is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        self.make_dirs(path)
    }

    /// Moves a file or a folder, which is also how it gets another name. Never
    /// onto something that is there already.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let (from, to) = (inside(from)?, inside(to)?);
        self.dir.symlink_metadata(from)?;
        if self.dir.symlink_metadata(to).is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        self.make_dirs(to.parent().unwrap_or(Path::new(NO_PARENT)))?;
        self.dir.rename(from, &self.dir, to)
    }

    /// Deletes a file, or a folder with all that is in it. Of a link it is the
    /// link that goes, and not what it leads to.
    pub fn remove(&self, path: &str) -> io::Result<()> {
        let path = inside(path)?;
        if self.dir.symlink_metadata(path)?.is_dir() {
            self.dir.remove_dir_all(path)
        } else {
            self.dir.remove_file(path)
        }
    }

    /// How much of the disk the directory takes, in bytes: what is set aside
    /// for its files, which is what deleting them would give back.
    pub fn used(&self) -> u64 {
        let mut total = 0;
        let _ = self.walk(Path::new("."), |_, metadata| {
            total += metadata.blocks() * 512;
            Ok(())
        });
        total
    }

    /// How much room is left on the disk the directory is on, in bytes.
    pub fn free(&self) -> io::Result<u64> {
        let disk = rustix::fs::fstatvfs(&self.dir)?;
        Ok(disk.f_bavail.saturating_mul(disk.f_frsize))
    }

    /// Packs `names`, each of them something in `folder`, into a gzipped tar
    /// at `to`. The error is `StorageFull` once the archive would be longer
    /// than `at_most` bytes; nothing is left of it then.
    pub fn pack(&self, folder: &str, names: &[String], to: &str, at_most: u64) -> io::Result<()> {
        let folder = at(folder)?;
        let (file, part) = self.begin(to)?;
        let packed = (|| -> io::Result<()> {
            let limited = Limited {
                to: file,
                left: at_most,
            };
            // Fast, not small: a world is mostly files that are packed already.
            let mut tar = tar::Builder::new(GzEncoder::new(limited, Compression::fast()));
            for name in names {
                let path = folder.join(inside(name)?);
                let metadata = self.dir.symlink_metadata(&path)?;
                self.add(&mut tar, &path, folder, &metadata)?;
                if metadata.is_dir() {
                    self.walk(&path, |inner, metadata| {
                        self.add(&mut tar, inner, folder, metadata)
                    })?;
                }
            }
            tar.into_inner()?.finish()?;
            Ok(())
        })();
        match packed {
            Ok(()) => self.finish(&part, to),
            Err(error) => {
                self.abandon(&part);
                Err(error)
            }
        }
    }

    /// Writes everything in the directory to `to` as a tar packed with
    /// Zstandard: a backup. Returns how long it came to be. The error is
    /// `StorageFull` once it would be longer than `at_most` bytes.
    ///
    /// A server that runs meanwhile changes files as they are read. Each file
    /// goes in at the length it had when it was come to; a game that keeps its
    /// world in many files is best told to save, and to hold off saving, first.
    pub fn back_up(&self, to: std::fs::File, at_most: u64) -> io::Result<u64> {
        let limited = Limited { to, left: at_most };
        let top = Path::new(".");
        let mut tar = tar::Builder::new(zstd::Encoder::new(limited, BACKUP_LEVEL)?);
        self.walk(top, |path, metadata| {
            self.add(&mut tar, path, top, metadata)
        })?;
        let limited = tar.into_inner()?.finish()?;
        limited.to.sync_all()?;
        Ok(at_most - limited.left)
    }

    /// Makes the directory what a backup says it was: everything in it goes,
    /// and what [`ServerDir::back_up`] wrote to `from` is put in its place.
    /// `keep_free` is how much of the disk must stay free while that is done.
    pub fn restore(&self, from: std::fs::File, keep_free: u64) -> io::Result<Unpacked> {
        // Read through once before anything is touched: a backup that cannot
        // be read to its end must not cost the files that are there.
        let mut whole = tar::Archive::new(zstd::Decoder::new(&from)?);
        for entry in whole.entries()? {
            io::copy(&mut entry?, &mut io::sink())?;
        }
        (&from).seek(SeekFrom::Start(0))?;

        for entry in self.dir.entries()? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                self.dir.remove_dir_all(entry.file_name())?;
            } else {
                self.dir.remove_file(entry.file_name())?;
            }
        }
        let mut unpacking = Unpacking {
            files: self,
            folder: PathBuf::new(),
            left: self.free()?.saturating_sub(keep_free),
            made: HashSet::new(),
            done: Unpacked {
                files: 0,
                skipped: 0,
            },
        };
        unpacking.tar(zstd::Decoder::new(from)?)?;
        Ok(unpacking.done)
    }

    /// Unpacks a zip or a tar, the tar as it is or packed with gzip or
    /// Zstandard, into the folder it is in, over what is there by the same
    /// names. Which it is, is read off its first bytes and not off its name:
    /// `Unsupported` if it is none of them. The error is `StorageFull` once
    /// more than `at_most` bytes would have been written; what was unpacked
    /// until then stays.
    pub fn unpack(&self, archive: &str, at_most: u64) -> io::Result<Unpacked> {
        let folder = inside(archive)?
            .parent()
            .unwrap_or(Path::new(NO_PARENT))
            .to_owned();
        let (mut file, _) = self.file(archive)?;
        let mut start = Vec::new();
        (&file).take(512).read_to_end(&mut start)?;
        file.seek(SeekFrom::Start(0))?;
        let mut unpacking = Unpacking {
            files: self,
            folder,
            left: at_most,
            made: HashSet::new(),
            done: Unpacked {
                files: 0,
                skipped: 0,
            },
        };
        let file = BufReader::new(file);
        match start.as_slice() {
            [b'P', b'K', ..] => unpacking.zip(file)?,
            [0x1f, 0x8b, ..] => unpacking.tar(GzDecoder::new(file))?,
            [0x28, 0xb5, 0x2f, 0xfd, ..] => unpacking.tar(zstd::Decoder::new(file)?)?,
            start if start.get(257..262) == Some(&b"ustar"[..]) => unpacking.tar(file)?,
            _ => return Err(io::ErrorKind::Unsupported.into()),
        }
        Ok(unpacking.done)
    }

    /// Gives a file or a directory to the server's user.
    fn own(&self, file: impl AsFd) -> io::Result<()> {
        fchown(file, Some(self.uid), Some(self.gid))
    }

    /// Makes a directory and those it sits in, where the server has not made
    /// them yet, and gives them to the server's user.
    fn make_dirs(&self, path: &Path) -> io::Result<()> {
        if path.as_os_str().is_empty() {
            return Ok(());
        }
        self.dir.create_dir_all(path)?;
        for made in path.ancestors().filter(|made| !made.as_os_str().is_empty()) {
            // Opened as a file: a directory opened as one is only a path
            // to the kernel, and it will not change the owner of a path.
            self.own(self.dir.open(made)?)?;
        }
        Ok(())
    }

    /// Calls `each` for everything under `top`, a folder before what is in it.
    /// A link is told of and not followed. What cannot be read, or goes while
    /// this runs, is passed over: a running server deletes files as it pleases.
    fn walk(
        &self,
        top: &Path,
        mut each: impl FnMut(&Path, &Metadata) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut folders = vec![top.to_owned()];
        while let Some(folder) = folders.pop() {
            let Ok(entries) = self.dir.read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let path = folder.join(entry.file_name());
                each(&path, &metadata)?;
                if metadata.is_dir() {
                    folders.push(path);
                }
            }
        }
        Ok(())
    }

    /// Adds a file, a folder or a link to an archive, under its path from
    /// `folder` on. Anything else a server may have left here is passed over,
    /// and so is what has gone since it was listed.
    fn add(
        &self,
        tar: &mut tar::Builder<impl Write>,
        path: &Path,
        folder: &Path,
        metadata: &Metadata,
    ) -> io::Result<()> {
        let named = path.strip_prefix(folder).unwrap_or(path);
        let mut header = Header::new_gnu();
        header.set_mode(metadata.mode() & 0o7777);
        header.set_mtime(metadata.mtime().max(0) as u64);
        header.set_size(0);
        match Kind::of(metadata) {
            Kind::Folder => {
                header.set_entry_type(EntryType::Directory);
                tar.append_data(&mut header, named, io::empty())
            }
            Kind::Link => {
                let Ok(target) = self.dir.read_link_contents(path) else {
                    return Ok(());
                };
                header.set_entry_type(EntryType::Symlink);
                tar.append_link(&mut header, named, target)
            }
            Kind::File => {
                let Ok(file) = self.dir.open_with(path, &reading()) else {
                    return Ok(());
                };
                let Ok(size) = file.metadata().and_then(|now| ordinary(&now)) else {
                    return Ok(());
                };
                header.set_entry_type(EntryType::Regular);
                header.set_size(size);
                // The header has said how long the file is, and a running
                // server may make it longer or shorter while it is read. So it
                // is cut off at that length, or filled out to it with zeros.
                let exactly = file.take(size).chain(io::repeat(0)).take(size);
                tar.append_data(&mut header, named, exactly)
            }
            Kind::Other => Ok(()),
        }
    }
}

/// A file that takes so many bytes and no more.
struct Limited {
    to: std::fs::File,
    left: u64,
}

impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.left {
            return Err(io::ErrorKind::StorageFull.into());
        }
        let written = self.to.write(bytes)?;
        self.left -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.to.flush()
    }
}

/// An archive on its way out into a folder.
struct Unpacking<'a> {
    files: &'a ServerDir,
    /// The folder everything goes into.
    folder: PathBuf,
    /// How many more bytes may be written.
    left: u64,
    /// The folders seen to so far, so that each is made and given away once.
    made: HashSet<PathBuf>,
    done: Unpacked,
}

impl Unpacking<'_> {
    /// Where an entry goes, or nothing for a name that climbs or starts at the root.
    fn place(&self, name: &Path) -> Option<PathBuf> {
        let mut place = self.folder.clone();
        for part in name.components() {
            match part {
                Component::Normal(part) => place.push(part),
                Component::CurDir => {}
                _ => return None,
            }
        }
        Some(place)
    }

    fn folder(&mut self, place: &Path) -> io::Result<()> {
        if self.made.insert(place.to_owned()) {
            self.files.make_dirs(place)?;
        }
        Ok(())
    }

    fn file(&mut self, place: &Path, mode: Option<u32>, from: impl Read) -> io::Result<()> {
        self.folder(place.parent().unwrap_or(Path::new(NO_PARENT)))?;
        // A file that is there by that name goes first, as `tar` has it go,
        // and a new one takes its place: a link is then not written through,
        // and the new file is Homewarp's own to set up. A folder by that name
        // stays, and the entry is passed over.
        match self.files.dir.remove_file(place) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let mut file = self.files.dir.create(place)?.into_std();
        // Its mode first, and its owner last. Homewarp is root with few of
        // root's powers (deploy/compose.yml), and changing the mode of a file
        // that has become someone else's is not among them.
        if let Some(mode) = mode {
            file.set_permissions(Permissions::from_mode(mode & 0o777))?;
        }
        // One byte more than there is room for, to tell too much from just enough.
        let wrote = io::copy(&mut from.take(self.left.saturating_add(1)), &mut file)?;
        self.left = self
            .left
            .checked_sub(wrote)
            .ok_or(io::ErrorKind::StorageFull)?;
        self.files.own(&file)?;
        self.done.files += 1;
        Ok(())
    }

    /// A link is made as the archive has it, wherever it says it leads: no
    /// read or write here goes through one that leads out, and to the server
    /// it leads into the server's own container.
    fn link(&mut self, place: &Path, target: Option<Cow<'_, Path>>) -> io::Result<()> {
        let Some(target) = target else {
            self.done.skipped += 1;
            return Ok(());
        };
        self.folder(place.parent().unwrap_or(Path::new(NO_PARENT)))?;
        self.files.dir.symlink_contents(target, place)
    }

    /// An entry that cannot go where it says, because a link on the way leads
    /// out or because something else is there by that name, is passed over
    /// and counted. Anything else that goes wrong ends the unpacking.
    fn settle(&mut self, done: io::Result<()>) -> io::Result<()> {
        use io::ErrorKind::{AlreadyExists, IsADirectory, NotADirectory, PermissionDenied};
        match done {
            Err(error)
                if matches!(
                    error.kind(),
                    PermissionDenied | AlreadyExists | IsADirectory | NotADirectory
                ) =>
            {
                self.done.skipped += 1;
                Ok(())
            }
            done => done,
        }
    }

    fn zip(&mut self, file: impl Read + Seek) -> io::Result<()> {
        let mut archive = zip::ZipArchive::new(file)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let place = entry.enclosed_name().and_then(|name| self.place(&name));
            let done = match place {
                Some(place) if entry.is_dir() => self.folder(&place),
                Some(place) if !entry.is_symlink() => {
                    let mode = entry.unix_mode();
                    self.file(&place, mode, &mut entry)
                }
                _ => {
                    self.done.skipped += 1;
                    Ok(())
                }
            };
            self.settle(done)?;
        }
        Ok(())
    }

    fn tar(&mut self, from: impl Read) -> io::Result<()> {
        let mut archive = tar::Archive::new(from);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let place = self.place(&entry.path()?);
            let kind = entry.header().entry_type();
            let done = match place {
                Some(place) if kind.is_dir() => self.folder(&place),
                Some(place) if kind.is_file() => {
                    let mode = entry.header().mode().ok();
                    self.file(&place, mode, &mut entry)
                }
                Some(place) if kind.is_symlink() => self.link(&place, entry.link_name()?),
                _ => {
                    self.done.skipped += 1;
                    Ok(())
                }
            };
            self.settle(done)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::Path,
    };

    use tar::{EntryType, Header};

    use super::{Kind, ServerDir, Unpacked};

    /// A directory for a server inside a scratch one, opened as whoever runs
    /// the test: giving a file to yourself is always allowed.
    fn scratch() -> (tempfile::TempDir, ServerDir) {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("server");
        std::fs::create_dir(&root).unwrap();
        let owner = std::fs::metadata(&root).unwrap();
        let dir = ServerDir::open(&root, owner.uid(), owner.gid()).unwrap();
        (scratch, dir)
    }

    fn names(dir: &ServerDir, path: &str) -> Vec<String> {
        let entries = dir.list(path).unwrap();
        entries.into_iter().map(|entry| entry.name).collect()
    }

    #[test]
    fn reads_and_writes_inside_and_refuses_links_that_lead_out() {
        let (scratch, dir) = scratch();
        let root = scratch.path().join("server");
        let outside = scratch.path().join("outside.txt");
        std::fs::write(&outside, "host secret").unwrap();

        assert_eq!(dir.read_to_string("server.properties").unwrap(), None);
        dir.write("server.properties", "motd=hi\n").unwrap();
        assert_eq!(
            dir.read_to_string("server.properties").unwrap().as_deref(),
            Some("motd=hi\n")
        );

        // A config file may belong in a directory the server has yet to make.
        dir.write("config/deep/settings.json", "{}").unwrap();
        assert_eq!(
            dir.read_to_string("config/deep/settings.json")
                .unwrap()
                .as_deref(),
            Some("{}")
        );
        // But not in one that is a link to somewhere else.
        symlink(scratch.path(), root.join("elsewhere")).unwrap();
        assert!(dir.write("elsewhere/planted.txt", "x").is_err());
        assert!(!scratch.path().join("planted.txt").exists());

        symlink(&outside, root.join("absolute")).unwrap();
        symlink("../outside.txt", root.join("relative")).unwrap();
        for link in ["absolute", "relative", "../outside.txt"] {
            assert!(dir.read_to_string(link).is_err(), "read through {link}");
            assert!(
                dir.write(link, "overwritten").is_err(),
                "write through {link}"
            );
            // The same for what the file manager does.
            assert!(dir.file(link).is_err(), "open through {link}");
            assert!(dir.text(link, 1 << 20).is_err(), "text through {link}");
        }
        assert!(dir.list("elsewhere").is_err());
        assert!(dir.begin("elsewhere/planted.txt").is_err());
        assert!(dir.make_dir("elsewhere/planted").is_err());
        assert!(
            dir.rename("server.properties", "elsewhere/taken.txt")
                .is_err()
        );
        assert!(!scratch.path().join("planted.txt").exists());
        assert!(!scratch.path().join("planted").exists());
        assert!(!scratch.path().join("taken.txt").exists());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "host secret");

        // Deleting a link deletes the link.
        dir.remove("absolute").unwrap();
        dir.remove("elsewhere").unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "host secret");
        assert!(scratch.path().join("server").exists());
    }

    #[test]
    fn a_file_of_settings_is_an_ordinary_file_of_a_size_and_nothing_else_is_opened() {
        use rustix::fs::{CWD, FileType, Mode, mknodat};

        let (scratch, dir) = scratch();
        let root = scratch.path().join("server");

        // Longer than settings are: not read, however much of it there is.
        let long = std::fs::File::create(root.join("long.properties")).unwrap();
        long.set_len(super::LARGEST_SETTINGS + 1).unwrap();
        let refused = dir.read_to_string("long.properties").unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::FileTooLarge);

        // A pipe by the name of a file: said to be no file, at once, by
        // reading and by writing alike. Neither waits for its other end.
        let pipe = root.join("pipe.properties");
        mknodat(CWD, &pipe, FileType::Fifo, Mode::from_raw_mode(0o644), 0).unwrap();
        assert!(dir.read_to_string("pipe.properties").is_err());
        assert!(dir.file("pipe.properties").is_err());
        let refused = dir.write("pipe.properties", "motd=hi\n").unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            dir.stat("pipe.properties", false).unwrap().kind,
            Kind::Other
        );

        // A folder by that name is no file either.
        std::fs::create_dir(root.join("folder.properties")).unwrap();
        assert!(dir.read_to_string("folder.properties").is_err());
        assert!(dir.write("folder.properties", "motd=hi\n").is_err());

        // A file that is written takes the place of the one before it, and
        // keeps what that one could be done with.
        dir.write("start.sh", "echo one\n").unwrap();
        std::fs::set_permissions(
            root.join("start.sh"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        dir.write("start.sh", "echo two\n").unwrap();
        assert_eq!(
            dir.read_to_string("start.sh").unwrap().as_deref(),
            Some("echo two\n")
        );
        let mode = std::fs::metadata(root.join("start.sh")).unwrap().mode();
        assert_eq!(mode & 0o777, 0o750);
        // Nothing of the writing is left beside it.
        assert!(!names(&dir, "").iter().any(|name| name.ends_with(".part")));
    }

    #[test]
    fn lists_makes_moves_and_deletes() {
        let (scratch, dir) = scratch();
        dir.write("b.txt", "bee").unwrap();
        dir.write("A.txt", "ay").unwrap();
        dir.make_dir("world/region").unwrap();
        dir.write("world/level.dat", "12345").unwrap();

        let top = dir.list("").unwrap();
        let seen: Vec<(&str, Kind, u64)> = top
            .iter()
            .map(|entry| (entry.name.as_str(), entry.kind, entry.size))
            .collect();
        assert_eq!(
            seen,
            [
                ("world", Kind::Folder, 0),
                ("A.txt", Kind::File, 2),
                ("b.txt", Kind::File, 3)
            ]
        );
        assert!(top[1].modified > 1_700_000_000);
        // A path may come with slashes at either end, and the top by any of its names.
        assert_eq!(names(&dir, "/world/"), ["region", "level.dat"]);
        assert_eq!(names(&dir, "/").len(), 3);
        assert_eq!(
            dir.list("b.txt").unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
        assert_eq!(
            dir.list("nowhere").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );

        assert_eq!(dir.text("world/level.dat", 5).unwrap(), "12345");
        assert_eq!(
            dir.text("world/level.dat", 4).unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
        assert_eq!(
            dir.text("world", 5).unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );

        // A file on its way in is not the file until it is whole, and then
        // it is what the old one was: one that may be run, here.
        let level = scratch.path().join("server/world/level.dat");
        std::fs::set_permissions(&level, PermissionsExt::from_mode(0o750)).unwrap();
        let (mut file, part) = dir.begin("world/level.dat").unwrap();
        file.write_all(b"new").unwrap();
        assert_eq!(dir.text("world/level.dat", 5).unwrap(), "12345");
        dir.finish(&part, "world/level.dat").unwrap();
        assert_eq!(dir.text("world/level.dat", 5).unwrap(), "new");
        assert_eq!(std::fs::metadata(&level).unwrap().mode() & 0o777, 0o750);
        let (_, part) = dir.begin("world/level.dat").unwrap();
        dir.abandon(&part);
        assert_eq!(names(&dir, "world"), ["region", "level.dat"]);
        assert_eq!(
            dir.begin("world").unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );

        assert_eq!(
            dir.rename("A.txt", "b.txt").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            dir.rename("nothing", "c.txt").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        dir.rename("A.txt", "kept/a.txt").unwrap();
        assert_eq!(names(&dir, "kept"), ["a.txt"]);
        assert_eq!(
            dir.make_dir("kept").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        dir.remove("world").unwrap();
        dir.remove("b.txt").unwrap();
        assert_eq!(names(&dir, ""), ["kept"]);
        // The directory itself is nobody's to delete, move or write over,
        // and no path climbs out of it.
        for path in ["", "/", ".", "..", "../server", "kept/../.."] {
            assert!(dir.remove(path).is_err(), "remove {path:?}");
            assert!(dir.rename(path, "moved").is_err(), "move {path:?}");
            assert!(dir.begin(path).is_err(), "write {path:?}");
        }
        assert!(dir.list("..").is_err());
        assert!(dir.used() > 0);
        assert!(dir.free().unwrap() > 0);
    }

    #[test]
    fn packs_what_is_chosen_and_unpacks_it_as_it_was() {
        let (_scratch, dir) = scratch();
        dir.write("saves/world/level.dat", "level").unwrap();
        dir.write("saves/world/region/r.0.0.mca", "region").unwrap();
        dir.write("saves/notes.txt", "notes").unwrap();
        dir.write("saves/left-out.txt", "not chosen").unwrap();
        dir.make_dir("saves/world/empty").unwrap();
        dir.dir
            .symlink_contents("level.dat", "saves/world/latest")
            .unwrap();

        let chosen = ["world".to_owned(), "notes.txt".to_owned()];
        // No room for it: nothing is left behind, not even a part of it.
        assert_eq!(
            dir.pack("saves", &chosen, "saves/out.tar.gz", 16)
                .unwrap_err()
                .kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(names(&dir, "saves").len(), 3);
        // A name that is a path out of the folder is not a name in it.
        assert!(
            dir.pack("saves", &["../saves".to_owned()], "x.tar.gz", 1 << 20)
                .is_err()
        );
        dir.pack("saves", &chosen, "saves/out.tar.gz", 1 << 20)
            .unwrap();

        dir.rename("saves/out.tar.gz", "restored/out.tar.gz")
            .unwrap();
        assert_eq!(
            dir.unpack("restored/out.tar.gz", 1 << 20).unwrap(),
            Unpacked {
                files: 3,
                skipped: 0
            }
        );
        assert_eq!(
            names(&dir, "restored"),
            ["world", "notes.txt", "out.tar.gz"]
        );
        assert_eq!(
            names(&dir, "restored/world"),
            ["empty", "region", "latest", "level.dat"]
        );
        assert_eq!(dir.text("restored/world/level.dat", 99).unwrap(), "level");
        assert_eq!(dir.text("restored/world/latest", 99).unwrap(), "level");
        assert_eq!(
            dir.text("restored/world/region/r.0.0.mca", 99).unwrap(),
            "region"
        );

        // Unpacked again with room for less than is in it, it stops.
        assert_eq!(
            dir.unpack("restored/out.tar.gz", 8).unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        // And what is not an archive is said to be none.
        assert_eq!(
            dir.unpack("saves/notes.txt", 1 << 20).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn unpacks_a_zip() {
        let (scratch, dir) = scratch();
        let options = zip::write::SimpleFileOptions::default();
        let file = std::fs::File::create(scratch.path().join("server/mods.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.add_directory("mods/", options).unwrap();
        zip.start_file("mods/one.jar", options).unwrap();
        zip.write_all(b"first").unwrap();
        zip.start_file("mods/config/two.toml", options.unix_permissions(0o600))
            .unwrap();
        zip.write_all(b"second").unwrap();
        zip.finish().unwrap();

        assert_eq!(
            dir.unpack("mods.zip", 1 << 20).unwrap(),
            Unpacked {
                files: 2,
                skipped: 0
            }
        );
        assert_eq!(names(&dir, "mods"), ["config", "one.jar"]);
        assert_eq!(dir.text("mods/config/two.toml", 99).unwrap(), "second");
        let mode = std::fs::metadata(scratch.path().join("server/mods/config/two.toml"))
            .unwrap()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        // Unpacked over what is there, a file takes the place of the one by
        // its name. It is not written into it: not into what a link by that
        // name leads to, either.
        dir.remove("mods/one.jar").unwrap();
        dir.dir
            .symlink_contents("config/two.toml", "mods/one.jar")
            .unwrap();
        // And a folder by a file's name is left as it is.
        dir.rename("mods/config/two.toml", "mods/config/kept.toml")
            .unwrap();
        dir.make_dir("mods/config/two.toml").unwrap();
        assert_eq!(
            dir.unpack("mods.zip", 1 << 20).unwrap(),
            Unpacked {
                files: 1,
                skipped: 1
            }
        );
        assert_eq!(dir.text("mods/one.jar", 99).unwrap(), "first");
        assert_eq!(dir.text("mods/config/kept.toml", 99).unwrap(), "second");
        assert!(
            std::fs::symlink_metadata(scratch.path().join("server/mods/one.jar"))
                .unwrap()
                .is_file()
        );
        assert!(scratch.path().join("server/mods/config/two.toml").is_dir());

        assert_eq!(
            dir.unpack("mods.zip", 4).unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
    }

    #[test]
    fn a_backup_puts_everything_back_as_it_was() {
        let (scratch, dir) = scratch();
        dir.write("world/level.dat", "level").unwrap();
        dir.write("server.properties", "motd=hi\n").unwrap();
        dir.make_dir("logs").unwrap();
        dir.dir
            .symlink_contents("world/level.dat", "latest")
            .unwrap();
        let create = |name: &str| std::fs::File::create(scratch.path().join(name)).unwrap();
        let open = |name: &str| std::fs::File::open(scratch.path().join(name)).unwrap();

        let size = dir.back_up(create("backup.tar.zst"), 1 << 20).unwrap();
        assert_eq!(size, open("backup.tar.zst").metadata().unwrap().len());
        assert_eq!(
            dir.back_up(create("no-room.tar.zst"), 8)
                .unwrap_err()
                .kind(),
            io::ErrorKind::StorageFull
        );

        // What has changed since, been added and been lost is all undone.
        dir.write("world/level.dat", "newer").unwrap();
        dir.write("plugins/added.jar", "x").unwrap();
        dir.remove("server.properties").unwrap();
        assert_eq!(
            dir.restore(open("backup.tar.zst"), 0).unwrap(),
            Unpacked {
                files: 2,
                skipped: 0
            }
        );
        assert_eq!(
            names(&dir, ""),
            ["logs", "world", "latest", "server.properties"]
        );
        assert_eq!(dir.text("world/level.dat", 99).unwrap(), "level");
        assert_eq!(dir.text("latest", 99).unwrap(), "level");

        // A backup that cannot be read costs nothing of what is there.
        std::fs::write(scratch.path().join("broken.tar.zst"), "not a backup").unwrap();
        assert!(dir.restore(open("broken.tar.zst"), 0).is_err());
        let whole = std::fs::read(scratch.path().join("backup.tar.zst")).unwrap();
        std::fs::write(
            scratch.path().join("cut.tar.zst"),
            &whole[..whole.len() / 2],
        )
        .unwrap();
        assert!(dir.restore(open("cut.tar.zst"), 0).is_err());
        assert_eq!(names(&dir, "").len(), 4);

        // And the file manager unpacks a backup as it does any archive.
        std::fs::write(scratch.path().join("server/copy.tar.zst"), &whole).unwrap();
        dir.rename("copy.tar.zst", "into/copy.tar.zst").unwrap();
        assert_eq!(dir.unpack("into/copy.tar.zst", 1 << 20).unwrap().files, 2);
        assert_eq!(dir.text("into/world/level.dat", 99).unwrap(), "level");
    }

    /// An archive is somebody else's word for where files go.
    #[test]
    fn an_archive_cannot_write_outside() {
        let (scratch, dir) = scratch();
        let root = scratch.path().join("server");
        let mut tar = tar::Builder::new(Vec::new());
        let entry = |kind, name: &[u8], size| {
            let mut header = Header::new_gnu();
            // Written into the header as it is: the library would refuse to.
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_entry_type(kind);
            header.set_mode(0o644);
            header.set_size(size);
            header.set_cksum();
            header
        };
        tar.append(&entry(EntryType::Regular, b"../climbed.txt", 1), &b"x"[..])
            .unwrap();
        tar.append(&entry(EntryType::Regular, b"/tmp/rooted.txt", 1), &b"x"[..])
            .unwrap();
        // A link that leads out, and then a file written through it.
        let mut out = entry(EntryType::Symlink, b"out", 0);
        out.set_link_name("..").unwrap();
        out.set_cksum();
        tar.append(&out, io::empty()).unwrap();
        tar.append(&entry(EntryType::Regular, b"out/through.txt", 1), &b"x"[..])
            .unwrap();
        tar.append(&entry(EntryType::Fifo, b"pipe", 0), io::empty())
            .unwrap();
        tar.append(&entry(EntryType::Regular, b"good.txt", 4), &b"good"[..])
            .unwrap();
        std::fs::write(root.join("hostile.tar"), tar.into_inner().unwrap()).unwrap();

        let unpacked = dir.unpack("hostile.tar", 1 << 20).unwrap();
        assert_eq!(unpacked.files, 1);
        assert_eq!(unpacked.skipped, 4);
        assert_eq!(dir.text("good.txt", 99).unwrap(), "good");
        for planted in ["climbed.txt", "through.txt", "rooted.txt"] {
            assert!(!scratch.path().join(planted).exists(), "{planted}");
        }
        assert!(!Path::new("/tmp/rooted.txt").exists());
        assert!(dir.file("out/through.txt").is_err());
    }
}
