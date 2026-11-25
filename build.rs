//! - Renders the app icon from its source, packaging/tauriwa.svg, so no
//!   image files live in the repository. src/icons.rs embeds the results.
//! - Lets the binary find libcef.so: next to itself (packages, see
//!   packaging/bundle-cef.sh) and, for development builds, in the CEF
//!   distribution downloaded by cef-dll-sys.

use std::path::PathBuf;

const ICON_SVG: &str = "packaging/tauriwa.svg";

/// (file name in OUT_DIR, size in pixels)
const SIZES: [(&str, u32); 2] = [("app.png", 256), ("app-512.png", 512)];

fn main() {
    println!("cargo:rerun-if-changed={ICON_SVG}");
    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    if let Some(cef_dir) = std::env::var_os("DEP_CEF_DLL_WRAPPER_CEF_DIR") {
        // write it down for packaging/bundle-cef.sh
        let cef_dir = PathBuf::from(cef_dir);
        if std::env::var("PROFILE").as_deref() == Ok("debug") {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{}", cef_dir.display());
        }
        let target_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
            .ancestors()
            .nth(3)
            .map(PathBuf::from);
        if let Some(target_dir) = target_dir {
            let _ = std::fs::write(
                target_dir.join("cef-dir.txt"),
                cef_dir.to_string_lossy().as_bytes(),
            );
        }
    }

    let svg = std::fs::read(ICON_SVG).unwrap_or_else(|e| panic!("{ICON_SVG}: {e}"));
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .unwrap_or_else(|e| panic!("{ICON_SVG}: {e}"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));

    for (name, size) in SIZES {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("icon size");
        let scale = size as f32 / tree.size().width().max(tree.size().height());
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let png = pixmap.encode_png().expect("encode png");
        std::fs::write(out_dir.join(name), png).expect("write icon");
    }
}
