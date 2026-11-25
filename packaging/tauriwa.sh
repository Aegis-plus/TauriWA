#!/bin/sh
# /usr/bin/tauriwa: the app lives in /usr/lib/tauriwa, next to its Chromium runtime
exec /usr/lib/tauriwa/tauriwa "$@"
