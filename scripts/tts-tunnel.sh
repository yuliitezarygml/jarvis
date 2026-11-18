#!/bin/sh
# The local listener is held by OpenSSH. Closing this process closes the tunnel.
set -eu
exec ssh -NT -o ExitOnForwardFailure=yes -o ServerAliveInterval=20 -o ServerAliveCountMax=3 -L 127.0.0.1:18181:127.0.0.1:8181 "${JARVIS_SSH_HOST:-root@192.168.50.38}"
