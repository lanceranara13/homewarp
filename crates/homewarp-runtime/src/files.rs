use std::{
    io::{self, Write},
    os::unix::fs::fchown,
    path::Path,
};

use cap_std::{ambient_authority, fs::Dir};

/// A server's directory, opened so that nothing inside it, symbolic links
/// included, can lead a read or a write outside it.
///
/// A game server, its plugins and its install script all write here, and none of
/// them is trusted: a planted `server.properties -> /etc/shadow` must not turn a
/// config patch into a write to the host.
pub struct ServerDir {
    dir: Dir,
    uid: u32,
    gid: u32,
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

    /// The file's text, or `None` if there is no such file.
    pub fn read_to_string(&self, path: &str) -> io::Result<Option<String>> {
        match self.dir.read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Writes a file, making the directories it sits in if the server has not
    /// made them yet. Those are given to the server's user as the file is.
    pub fn write(&self, path: &str, contents: &str) -> io::Result<()> {
        let parent = Path::new(path).parent().unwrap_or(Path::new(""));
        if !parent.as_os_str().is_empty() {
            self.dir.create_dir_all(parent)?;
            for made in parent
                .ancestors()
                .filter(|made| !made.as_os_str().is_empty())
            {
                // Opened as a file: a directory opened as one is only a path
                // to the kernel, and it will not change the owner of a path.
                fchown(self.dir.open(made)?, Some(self.uid), Some(self.gid))?;
            }
        }
        let mut file = self.dir.create(path)?;
        file.write_all(contents.as_bytes())?;
        fchown(&file, Some(self.uid), Some(self.gid))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, symlink};

    use super::ServerDir;

    #[test]
    fn reads_and_writes_inside_and_refuses_links_that_lead_out() {
        let scratch = tempfile::tempdir().unwrap();
        let outside = scratch.path().join("outside.txt");
        std::fs::write(&outside, "host secret").unwrap();
        let root = scratch.path().join("server");
        std::fs::create_dir(&root).unwrap();
        // Chowning to yourself is always allowed, so the test needs no privilege.
        let owner = std::fs::metadata(&root).unwrap();
        let dir = ServerDir::open(&root, owner.uid(), owner.gid()).unwrap();

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
        }
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "host secret");
    }
}
