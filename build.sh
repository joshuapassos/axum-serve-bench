#!/usr/bin/env bash
# Release builds of the same server against axum main and the fix branch (both pinned revs).
set -eu
cd "$(dirname "$0")"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
for v in main fork; do
  (cd server-$v && CARGO_TARGET_DIR=../target-server-$v cargo build --release -q)
done
(cd client && CARGO_TARGET_DIR=../target-client cargo build --release -q)
