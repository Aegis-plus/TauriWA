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

if [ ! -x "$release/tauriwa" ]; then
    echo "bundle-cef: run 'cargo build --release' first" >&2
    exit 1
fi

# The CEF distribution the build used: in $CEF_PATH when set, otherwise
# downloaded by cef-dll-sys into its build directory. (Files the build
# writes elsewhere may be missing when the build comes from a cache.)
# "156.7.0+156.0.31" in Cargo.lock -> CEF "156.0.31", the directory name in $CEF_PATH
version=$(grep -A1 '^name = "cef-dll-sys"' Cargo.lock | sed -n 's/^version = ".*+\(.*\)"/\1/p')
find_libcef() {
    [ -d "$1" ] || return 0
    find "$1" -name libcef.so -path "*$2*" -printf '%h\n' 2>/dev/null | while read -r dir; do
        if [ -f "$dir/icudtl.dat" ] && [ -d "$dir/locales" ]; then
            echo "$dir"
            break
        fi
    done
}
cef_dir=$(find_libcef "${CEF_PATH:-/nonexistent}" "/$version/")
[ -n "$cef_dir" ] || cef_dir=$(find_libcef "$release/build" cef-dll-sys)
[ -n "$cef_dir" ] || cef_dir=$(cat "$release/cef-dir.txt" 2>/dev/null || true)
if [ ! -f "$cef_dir/libcef.so" ]; then
    echo "bundle-cef: CEF distribution not found (CEF_PATH=${CEF_PATH:-unset})" >&2
    exit 1
fi
echo "bundle-cef: using $cef_dir"

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
