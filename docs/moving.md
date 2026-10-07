# Moving a server, and getting one back

A server in Homewarp is two things: the template it was made from, and its
files. Both can be carried by hand. That is how a server is got back after a
lost disk, and how one is moved over from another panel.

## Getting a server back from the store

With a store set (**Settings**, *A store elsewhere for backups*), every backup
is copied to your bucket as it is made. A copy is the backup's own file, a tar
packed with Zstandard, named for the server and the moment it was made:

```
<folder>/<server's name>-<eight characters>/2026-10-07-035327-1.tar.zst
```

Homewarp keeps its list of backups on the same disk as the servers. If that
disk is lost, the list is lost with it, and the copies in the bucket are what
is left. To get a server back on a new Homewarp:

1. Fetch the copy from the bucket with whatever speaks to it: the store's own
   web page, `aws s3 cp`, `mc cp`, `rclone`.
2. Import the template the server was made from (**Templates**, **Import**),
   and make a server from it. Let it install, then stop it.
3. Under the server's **Files**, upload the copy and choose **Unpack**. It is
   unpacked into the folder it is in, over what is there by the same names.
4. Delete the archive, and start the server.

This was tried on 2026-10-07 with a 300 MB world: fetched from the store with
`curl`, uploaded, unpacked, and the same as the original byte for byte.

A backup of more than 5 GB is not copied: that is the most a store takes in
one piece, and sending one in parts is not built. Such a backup says so on the
**Backups** tab, and stays where it was made.

## Moving a server over from Pterodactyl or Pelican

There is no importer that reads another panel's database. A server is moved
the way one is got back, with that panel's own backup as the archive:

1. In the old panel, export the server's egg (as JSON or YAML) and make a
   backup of the server. Download both. A backup there is a tar packed with
   gzip, which Homewarp unpacks as well.
2. Import the egg in Homewarp. Eggs are read as those two panels write them.
3. Make a server from it with the same port and the same answers to what the
   egg asks. Let it install, then stop it.
4. Under **Files**, delete what the install put there if the backup has it
   all, upload the backup, and **Unpack**.
5. Start the server. What its template sets in its config files (the port,
   for one) is put in again at each start.

Only the last part of this was tried with a real archive, and that one was
Homewarp's own. The egg importer has been run over 116 published eggs, and a
tar packed with gzip is unpacked in the tests. A whole server has not been
carried over from a running Wings node: there was none here to carry one from.
