# Core as an image in the lab's home, where Core itself is a program and not a
# container. It listens from a container of its own image to probe the tunnel,
# and this is that image: the same program at the same path.
FROM scratch
COPY homewarp-static /usr/local/bin/homewarp
