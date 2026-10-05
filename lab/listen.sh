#!/bin/sh
# A service that must stay out of reach (SSH on the home host, a NAS), and on the
# client a stand-in for the internet. Answers with the address the caller came from.
exec socat TCP4-LISTEN:"${SVC:-2222}",fork,reuseaddr SYSTEM:'echo reached $SOCAT_PEERADDR'
