#!/bin/sh
set -eu
exec node "$(dirname "$0")/kb/fetch.mjs" "$@"
