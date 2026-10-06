//! Nothing that is asked of a server's files reaches past them (PLAN.md §6;
//! §11, Phase 5).
//!
//! Each try here is made in a small world of its own: a server's folder with
//! ordinary things in it and with links that lead out of it, and beside it what
//! a home machine has that a server must never touch. Every thing the file
//! layer can do is tried with every path that could lead out.
//!
//! What is held to is not that each try fails. Some are harmless: a link is
//! deleted as a link, an odd name is just a name. It is that afterwards the
//! outside is exactly as it was, and that nothing that came back was the
//! outside's: not what its files hold, not what they are called, not how long
//! they are.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
};

use homewarp_runtime::{How, Kind, ServerDir};
use tar::{EntryType, Header};

/// What the outside holds, which nothing a server is asked may read.
const SECRET: &str = "the home machine's own secret";
/// Where a try that got all the way out would leave its mark on this machine.
const PLANTED: &str = "/tmp/planted-by-the-traversal-suite";
const MOST: u64 = 1 << 20;

struct World {
    /// Kept for as long as the world is: dropping it deletes everything.
    scratch: tempfile::TempDir,
    dir: ServerDir,
}

impl World {
    fn root(&self) -> PathBuf {
        self.scratch.path().join("server")
    }

    fn outside(&self) -> PathBuf {
        self.scratch.path().join("outside")
    }
}

fn world() -> World {
    let scratch = tempfile::tempdir().unwrap();
    let (root, outside) = (
        scratch.path().join("server"),
        scratch.path().join("outside"),
    );
    fs::create_dir_all(outside.join("folder")).unwrap();
    fs::write(outside.join("secret.txt"), SECRET).unwrap();
    fs::write(outside.join("folder/kept.txt"), SECRET).unwrap();

    fs::create_dir_all(root.join("folder")).unwrap();
    fs::write(root.join("plain.txt"), "inside").unwrap();
    fs::write(root.join("folder/inner.txt"), "inside").unwrap();
    // Every way a link can lead out: to a file and to a folder, by a whole
    // path and by climbing, by way of another link, from further down, to
    // the top of the machine, and round to itself.
    symlink(outside.join("secret.txt"), root.join("out-file")).unwrap();
    symlink("../outside/secret.txt", root.join("out-relative")).unwrap();
    symlink(&outside, root.join("out-folder")).unwrap();
    symlink("..", root.join("out-up")).unwrap();
    symlink("out-folder", root.join("chain")).unwrap();
    symlink("../../outside", root.join("folder/back-out")).unwrap();
    symlink("/", root.join("root-link")).unwrap();
    symlink("loop", root.join("loop")).unwrap();

    let owner = fs::metadata(&root).unwrap();
    let dir = ServerDir::open(&root, owner.uid(), owner.gid()).unwrap();
    World { scratch, dir }
}

/// Everything beside the server's folder, as it is: what each thing is, its
/// mode, and what is in it or where it points.
fn outside_of(world: &World) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    fn walk(at: &Path, skip: &Path, all: &mut BTreeMap<PathBuf, (u32, Vec<u8>)>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path == skip {
                continue;
            }
            let about = fs::symlink_metadata(&path).unwrap();
            let holds = if about.is_symlink() {
                fs::read_link(&path)
                    .unwrap()
                    .into_os_string()
                    .into_encoded_bytes()
            } else if about.is_file() {
                fs::read(&path).unwrap()
            } else {
                Vec::new()
            };
            all.insert(path.clone(), (about.mode(), holds));
            if about.is_dir() {
                walk(&path, skip, all);
            }
        }
    }
    let mut all = BTreeMap::new();
    walk(world.scratch.path(), &world.root(), &mut all);
    all
}

/// The paths that could lead out, by every trick there is.
fn hostile() -> Vec<String> {
    let mut paths: Vec<String> = [
        // Saying outright that it means to climb.
        "..",
        "../outside/secret.txt",
        "../../etc/passwd",
        "folder/../../outside/secret.txt",
        "./../outside/secret.txt",
        "plain.txt/../../outside/secret.txt",
        // From the top of the machine.
        "/etc/passwd",
        "//etc/passwd",
        "/../outside/secret.txt",
        // By a link, and to what is behind it.
        "out-file",
        "out-relative",
        "out-folder",
        "out-folder/",
        "out-folder/secret.txt",
        "out-folder/folder/kept.txt",
        "out-folder/planted.txt",
        "out-folder/folder",
        "out-up",
        "out-up/outside/secret.txt",
        "out-up/planted.txt",
        "chain",
        "chain/secret.txt",
        "chain/planted.txt",
        "folder/back-out",
        "folder/back-out/secret.txt",
        "folder/back-out/planted.txt",
        "root-link",
        "root-link/etc/passwd",
        "root-link/tmp/planted-by-the-traversal-suite",
        "loop",
        "loop/x",
        // Out by a link and in again by the name of the folder.
        "out-up/server/plain.txt",
        // Through what is no folder.
        "plain.txt/x",
        // Names with something in them that ends a name somewhere else.
        "a\0b",
        "..\0",
        "out-file\0.txt",
        "out-folder\0/secret.txt",
        // Dots and strokes that are not the ones the file system reads.
        "..\\outside\\secret.txt",
        "\u{ff0e}\u{ff0e}/outside/secret.txt",
        "%2e%2e/outside/secret.txt",
        "..%2foutside%2fsecret.txt",
        // The folder itself, in every way of saying it.
        "",
        ".",
        "./",
        "/",
    ]
    .map(str::to_owned)
    .to_vec();
    // Longer than any path the machine takes.
    paths.push("a/".repeat(3000));
    paths.push(format!(
        "{}/../../outside/secret.txt",
        "folder/..".repeat(400)
    ));
    paths
}

/// What is read off an open file, if anything can be.
fn read(file: io::Result<fs::File>) -> Vec<String> {
    let mut text = Vec::new();
    match file {
        Ok(mut file) => {
            let _ = file.read_to_end(&mut text);
            vec![String::from_utf8_lossy(&text).into_owned()]
        }
        Err(_) => Vec::new(),
    }
}

/// What is written to an open file, if anything can be.
fn write(file: io::Result<fs::File>) -> Vec<String> {
    if let Ok(mut file) = file {
        let _ = file.write_all(b"planted");
    }
    Vec::new()
}

/// What an archive that was packed holds, unpacked from its gzip: its names
/// and its files' contents, all in one.
fn packed(world: &World, name: &str) -> Vec<String> {
    let Ok(file) = fs::File::open(world.root().join(name)) else {
        return Vec::new();
    };
    let mut plain = Vec::new();
    let _ = flate2::read::GzDecoder::new(file).read_to_end(&mut plain);
    vec![String::from_utf8_lossy(&plain).into_owned()]
}

/// One thing the file layer does, tried with a path. It returns whatever it
/// learned by it: what it read, what it saw listed, how long something was.
type Try = fn(&World, &str) -> Vec<String>;

fn tries() -> Vec<(&'static str, Try)> {
    vec![
        ("read_to_string", |world, path| {
            world
                .dir
                .read_to_string(path)
                .ok()
                .flatten()
                .into_iter()
                .collect()
        }),
        ("write", |world, path| {
            let _ = world.dir.write(path, "planted");
            Vec::new()
        }),
        ("list", |world, path| match world.dir.list(path) {
            Ok(entries) => entries
                .into_iter()
                .map(|entry| format!("name {} length {}", entry.name, entry.size))
                .collect(),
            Err(_) => Vec::new(),
        }),
        ("stat, following", |world, path| {
            match world.dir.stat(path, true) {
                Ok(stat) if stat.kind == Kind::File => vec![format!("length {}", stat.size)],
                _ => Vec::new(),
            }
        }),
        ("stat, not following", |world, path| {
            match world.dir.stat(path, false) {
                Ok(stat) if stat.kind == Kind::File => vec![format!("length {}", stat.size)],
                _ => Vec::new(),
            }
        }),
        ("open to read", |world, path| {
            let how = How {
                read: true,
                ..How::default()
            };
            read(world.dir.open_with(path, how))
        }),
        ("open to write", |world, path| {
            let how = How {
                write: true,
                create: true,
                truncate: true,
                ..How::default()
            };
            write(world.dir.open_with(path, how))
        }),
        ("open to append", |world, path| {
            let how = How {
                write: true,
                append: true,
                ..How::default()
            };
            write(world.dir.open_with(path, how))
        }),
        ("open to make", |world, path| {
            let how = How {
                write: true,
                new: true,
                ..How::default()
            };
            write(world.dir.open_with(path, how))
        }),
        ("open to read and write", |world, path| {
            let how = How {
                read: true,
                write: true,
                ..How::default()
            };
            read(world.dir.open_with(path, how))
        }),
        ("unlink", |world, path| {
            let _ = world.dir.unlink(path);
            Vec::new()
        }),
        ("remove_empty_dir", |world, path| {
            let _ = world.dir.remove_empty_dir(path);
            Vec::new()
        }),
        ("file", |world, path| {
            read(world.dir.file(path).map(|(file, _)| file))
        }),
        ("file, for its length", |world, path| {
            match world.dir.file(path) {
                Ok((_, length)) => vec![format!("length {length}")],
                Err(_) => Vec::new(),
            }
        }),
        ("text", |world, path| {
            world.dir.text(path, MOST).into_iter().collect()
        }),
        ("begin, then finish", |world, path| {
            if let Ok((mut file, part)) = world.dir.begin(path) {
                let _ = file.write_all(b"planted");
                drop(file);
                let _ = world.dir.finish(&part, path);
            }
            Vec::new()
        }),
        ("finish onto it", |world, path| {
            if let Ok((mut file, part)) = world.dir.begin("upload.txt") {
                let _ = file.write_all(b"planted");
                drop(file);
                let _ = world.dir.finish(&part, path);
            }
            Vec::new()
        }),
        // A link moved is still a link, and is read as one: by the file layer,
        // which is the only way a server's files are ever read.
        ("finish from it", |world, path| {
            let _ = world.dir.finish(path, "taken.txt");
            world.dir.text("taken.txt", MOST).into_iter().collect()
        }),
        ("abandon", |world, path| {
            world.dir.abandon(path);
            Vec::new()
        }),
        ("make_dir", |world, path| {
            let _ = world.dir.make_dir(path);
            Vec::new()
        }),
        // Moved in, a thing from outside could then be read as the server's own.
        ("rename from it", |world, path| {
            let _ = world.dir.rename(path, "moved");
            let mut learned = world
                .dir
                .text("moved", MOST)
                .into_iter()
                .collect::<Vec<_>>();
            learned.extend(world.dir.text("moved/secret.txt", MOST));
            learned
        }),
        ("rename to it", |world, path| {
            let _ = world.dir.rename("plain.txt", path);
            Vec::new()
        }),
        ("rename a folder to it", |world, path| {
            let _ = world.dir.rename("folder", path);
            Vec::new()
        }),
        ("remove", |world, path| {
            let _ = world.dir.remove(path);
            Vec::new()
        }),
        ("pack that folder", |world, path| {
            let names = ["secret.txt", "plain.txt", "folder"].map(str::to_owned);
            let _ = world.dir.pack(path, &names, "packed.tar.gz", MOST);
            packed(world, "packed.tar.gz")
        }),
        ("pack it by name", |world, path| {
            let _ = world
                .dir
                .pack("", &[path.to_owned()], "packed.tar.gz", MOST);
            packed(world, "packed.tar.gz")
        }),
        ("pack into it", |world, path| {
            let _ = world.dir.pack("", &["plain.txt".to_owned()], path, MOST);
            Vec::new()
        }),
        ("unpack it", |world, path| {
            let _ = world.dir.unpack(path, MOST);
            Vec::new()
        }),
    ]
}

/// Whether something that came back is the outside's.
fn leaks(learned: &str) -> bool {
    learned.contains(SECRET)
        // What the files outside are called, and how long they are.
        || ["name secret.txt", "name kept.txt", "name outside ", "name etc ", "name passwd "]
            .iter()
            .any(|name| learned.contains(name))
        || learned == format!("length {}", SECRET.len())
        // A line of the machine's own list of accounts.
        || learned.contains("root:x:0:0")
}

#[test]
fn nothing_done_with_a_path_that_leads_out_reaches_the_outside() {
    let _ = fs::remove_file(PLANTED);
    let (paths, tries) = (hostile(), tries());
    let mut tried = 0;
    for path in &paths {
        for (what, attempt) in &tries {
            let world = world();
            let before = outside_of(&world);
            let learned = attempt(&world, path);
            let shown: String = path.chars().take(60).collect();
            assert_eq!(
                outside_of(&world),
                before,
                "{what} with {shown:?} changed what is outside"
            );
            assert!(
                !Path::new(PLANTED).exists(),
                "{what} with {shown:?} wrote to the machine itself"
            );
            for learned in &learned {
                assert!(
                    !leaks(learned),
                    "{what} with {shown:?} came back with what is outside: {learned:?}"
                );
            }
            tried += 1;
        }
    }
    // Every path with every thing there is to do: none was passed over.
    assert_eq!(tried, paths.len() * tries.len());
    assert!(tried > 1000, "{tried}");
}

/// The suite is worth what it would catch. So the same checks are held
/// against a file layer that does it the easy way, joining the path on and
/// asking the machine: every kind of try has to be caught doing harm.
#[test]
fn the_same_checks_catch_a_file_layer_that_trusts_the_path() {
    let world = world();
    let before = outside_of(&world);
    // Read, the easy way: the secret comes back, and is seen to be one.
    let trusted = fs::read_to_string(world.root().join("out-folder/secret.txt")).unwrap();
    assert!(leaks(&trusted));
    assert!(leaks(&format!(
        "length {}",
        fs::metadata(world.root().join("out-file")).unwrap().len()
    )));
    let listed = fs::read_dir(world.root().join("out-folder"))
        .unwrap()
        .map(|entry| {
            format!(
                "name {} length 0",
                entry.unwrap().file_name().to_string_lossy()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(leaks(&listed));
    // Written, the easy way: the outside is no longer as it was.
    fs::write(world.root().join("out-folder/planted.txt"), "planted").unwrap();
    assert_ne!(outside_of(&world), before);
    fs::remove_file(world.outside().join("planted.txt")).unwrap();
    assert_eq!(outside_of(&world), before);
    fs::write(world.root().join("out-relative"), "overwritten").unwrap();
    assert_ne!(outside_of(&world), before);
}

/// A tar entry with this name written into it as it is. The library would
/// refuse to write a name that climbs.
fn entry(kind: EntryType, name: &[u8], size: u64) -> Header {
    let mut header = Header::new_gnu();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_entry_type(kind);
    header.set_mode(0o644);
    header.set_size(size);
    header.set_cksum();
    header
}

fn link(kind: EntryType, name: &[u8], to: &str) -> Header {
    let mut header = entry(kind, name, 0);
    header.as_old_mut().linkname[..to.len()].copy_from_slice(to.as_bytes());
    header.set_cksum();
    header
}

#[test]
fn an_archive_reaches_no_further_than_a_path_does() {
    let _ = fs::remove_file(PLANTED);
    let world = world();
    let before = outside_of(&world);
    let outside = world.outside();
    let secret = outside.join("secret.txt");

    let mut tar = tar::Builder::new(Vec::new());
    let x = &b"planted"[..];
    for name in [
        &b"../climbed.txt"[..],
        b"../outside/secret.txt",
        b"folder/../../climbed.txt",
        b"/tmp/planted-by-the-traversal-suite",
        // Through the links that are there already.
        b"out-folder/planted.txt",
        b"out-folder/secret.txt",
        b"out-file",
        b"out-relative",
        b"chain/planted.txt",
        b"folder/back-out/planted.txt",
        b"root-link/tmp/planted-by-the-traversal-suite",
        b"out-up/planted.txt",
    ] {
        tar.append(&entry(EntryType::Regular, name, 7), x).unwrap();
    }
    // Links the archive brings itself, and then files written through them.
    tar.append(
        &link(EntryType::Symlink, b"new-out", outside.to_str().unwrap()),
        io::empty(),
    )
    .unwrap();
    tar.append(&entry(EntryType::Regular, b"new-out/planted.txt", 7), x)
        .unwrap();
    tar.append(
        &link(EntryType::Symlink, b"new-up", "../outside"),
        io::empty(),
    )
    .unwrap();
    tar.append(&entry(EntryType::Regular, b"new-up/secret.txt", 7), x)
        .unwrap();
    // A second name for a file outside, which writing to would write there.
    tar.append(
        &link(EntryType::Link, b"hard", secret.to_str().unwrap()),
        io::empty(),
    )
    .unwrap();
    tar.append(&entry(EntryType::Regular, b"hard", 7), x)
        .unwrap();
    tar.append(
        &link(EntryType::Link, b"hard-climbing", "../outside/secret.txt"),
        io::empty(),
    )
    .unwrap();
    // A folder where a link is, to be filled as if it were the server's own.
    tar.append(
        &entry(EntryType::Directory, b"out-folder/made", 0),
        io::empty(),
    )
    .unwrap();
    // Neither file nor folder nor link.
    tar.append(&entry(EntryType::Fifo, b"pipe", 0), io::empty())
        .unwrap();
    tar.append(&entry(EntryType::Char, b"device", 0), io::empty())
        .unwrap();
    tar.append(&entry(EntryType::Regular, b"good.txt", 7), x)
        .unwrap();
    fs::write(world.root().join("hostile.tar"), tar.into_inner().unwrap()).unwrap();

    let unpacked = world.dir.unpack("hostile.tar", MOST).unwrap();
    assert_eq!(world.dir.text("good.txt", MOST).unwrap(), "planted");
    assert!(unpacked.files >= 1, "{unpacked:?}");
    assert_eq!(outside_of(&world), before, "{unpacked:?}");
    assert!(!Path::new(PLANTED).exists());
    // Nothing the archive brought is a way to the secret afterwards.
    for path in [
        "hard",
        "hard-climbing",
        "new-out/secret.txt",
        "new-up/secret.txt",
        "out-file",
    ] {
        let read = world.dir.text(path, MOST).unwrap_or_default();
        assert!(!leaks(&read), "{path}: {read:?}");
    }

    // The same in a zip, whose names are the archive's own word as well.
    let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for name in [
        "../climbed.txt",
        "/tmp/planted-by-the-traversal-suite",
        "out-folder/planted.txt",
        "out-folder/secret.txt",
        "out-relative",
        "folder/back-out/planted.txt",
        "..\\climbed.txt",
        "fine.txt",
    ] {
        zip.start_file(name, stored).unwrap();
        zip.write_all(b"planted").unwrap();
    }
    // A link, as a zip made on such a machine carries one.
    zip.add_symlink("zipped-out", outside.to_str().unwrap(), stored)
        .unwrap();
    zip.start_file("zipped-out/planted.txt", stored).unwrap();
    zip.write_all(b"planted").unwrap();
    fs::write(
        world.root().join("hostile.zip"),
        zip.finish().unwrap().into_inner(),
    )
    .unwrap();

    let unpacked = world.dir.unpack("hostile.zip", MOST).unwrap();
    assert_eq!(world.dir.text("fine.txt", MOST).unwrap(), "planted");
    assert_eq!(outside_of(&world), before, "{unpacked:?}");
    assert!(!Path::new(PLANTED).exists());
}

#[test]
fn a_backup_put_back_reaches_no_further_either() {
    let world = world();
    let before = outside_of(&world);
    // A backup is a zstd tar of the folder, and whoever can put a file in
    // their server can put a backup of their own making where one is kept.
    let mut tar = tar::Builder::new(Vec::new());
    let x = &b"planted"[..];
    tar.append(&entry(EntryType::Regular, b"../climbed.txt", 7), x)
        .unwrap();
    tar.append(
        &link(
            EntryType::Symlink,
            b"way-out",
            world.outside().to_str().unwrap(),
        ),
        io::empty(),
    )
    .unwrap();
    tar.append(&entry(EntryType::Regular, b"way-out/planted.txt", 7), x)
        .unwrap();
    tar.append(&entry(EntryType::Regular, b"kept.txt", 7), x)
        .unwrap();
    let packed = zstd::encode_all(&tar.into_inner().unwrap()[..], 3).unwrap();
    let backup = world.scratch.path().join("hostile.backup");
    fs::write(&backup, packed).unwrap();
    let before_with_backup = outside_of(&world);
    assert_ne!(before, before_with_backup);

    // Refused whole, or put back without what climbs: either way the outside stays.
    let _ = world.dir.restore(fs::File::open(&backup).unwrap(), 0);
    assert_eq!(outside_of(&world), before_with_backup);
    assert!(!world.outside().join("planted.txt").exists());
    assert!(!world.scratch.path().join("climbed.txt").exists());
}
