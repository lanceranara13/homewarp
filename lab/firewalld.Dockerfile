# The simulated VPS where run.sh is asked for one with a firewall of its own
# (GATE_FW=firewalld): AlmaLinux, where firewalld is what a VPS comes with.
# Nothing starts it here. run.sh does, as the machine's services would.
FROM almalinux:9
RUN dnf -y -q install dbus-daemon firewalld iproute nftables openssl procps-ng socat wireguard-tools && dnf clean all
COPY game.sh listen.sh /lab/
RUN chmod 755 /lab/*.sh
CMD ["sleep", "infinity"]
