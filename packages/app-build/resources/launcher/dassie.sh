#!/bin/sh

BIN_FOLDER="$(dirname "$(readlink -f "$0")")"
DASSIE_ROOT="$(dirname "$BIN_FOLDER")"

export DASSIE_ROOT

"$BIN_FOLDER/node" "$DASSIE_ROOT/backend.mjs" "$@"