#!/bin/sh
# Assembles target/bundle/tauriwa/: the tauriwa binary next to the Chromium
# (CEF) runtime it needs (libcef.so, .pak resources, ICU data, V8 snapshot,
# SwiftShader, locales). The .deb/.rpm install that directory as
# /usr/lib/tauriwa, plus packaging/tauriwa.sh as /usr/bin/tauriwa; the
# .tar.xz ships it as is.
#
# Run after `cargo build --release`.
set -eu

cd "$(dirname "$0")/.."
release=target/release
out=target/bundle/tauriwa

# the CEF distribution used by the build, written down by build.rs
cef_dir=$(cat "$release/cef-dir.txt" 2>/dev/null || true)
if [ ! -x "$release/tauriwa" ] || [ ! -f "$cef_dir/libcef.so" ]; then
    echo "bundle-cef: run 'cargo build --release' first" >&2
    exit 1
fi

rm -rf "$out"
mkdir -p "$out"

cp "$release/tauriwa" "$out/"
for f in libcef.so libvk_swiftshader.so libvulkan.so.1 vk_swiftshader_icd.json \
    icudtl.dat v8_context_snapshot.bin resources.pak chrome_100_percent.pak \
    chrome_200_percent.pak; do
    cp "$cef_dir/$f" "$out/"
done
cp -r "$cef_dir/locales" "$out/"

# CEF ships libcef.so with debug symbols: ~1.5 GB -> ~270 MB
strip --strip-unneeded "$out/libcef.so" "$out/libvk_swiftshader.so" "$out/tauriwa"
chmod 644 "$out"/*.so "$out"/*.so.1 "$out"/*.pak "$out"/*.dat "$out"/*.bin "$out"/*.json "$out"/locales/*

du -sh "$out"
