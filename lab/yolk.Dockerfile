# What Core runs a lab server in: the lab's node, started the way an egg's image
# starts, by running the command Core hands it in $STARTUP.
FROM homewarp-lab-node
ENTRYPOINT ["/bin/sh", "-c", "cd /home/container && eval \"$STARTUP\""]
