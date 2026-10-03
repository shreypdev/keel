#!/bin/sh
# Runs the Swift consumer: it exits 1 when a check fails.
set -eu
exec "$1"
