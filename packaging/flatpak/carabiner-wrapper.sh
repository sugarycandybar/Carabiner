#!/usr/bin/env sh
set -eu
# Ensure Carabiner stores data under the sandboxed XDG data directory so
# the app does not need broad access to the user's $HOME.
XDG_DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CARABINER_DATA_DIR="${CARABINER_DATA_DIR:-$XDG_DATA_HOME/carabiner}"
export CARABINER_DATA_DIR
# Create the data dir if it does not exist
mkdir -p "$CARABINER_DATA_DIR"

cd /app/share
exec python3 /app/share/carabiner.py "$@"
