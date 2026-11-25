//! Renders the app icon from its source, packaging/tauriwa.svg, so no
//! image files live in the repository. src/icons.rs embeds the results.

use std::path::PathBuf;

const ICON_SVG: &str = "packaging/tauriwa.svg";

/// (file name in OUT_DIR, size in pixels)
const SIZES: [(&str, u32); 2] = [("app.png", 256), ("app-512.png", 512)];

fn main() {
    println!("cargo:rerun-if-changed={ICON_SVG}");
    println!("cargo:rerun-if-changed=build.rs");

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
