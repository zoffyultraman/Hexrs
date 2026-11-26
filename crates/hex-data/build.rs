use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_bundled_meta_db)");
    println!("cargo:rerun-if-changed=assets/meta.db.zst");
    let asset = Path::new("assets/meta.db.zst");
    if asset.is_file()
        && std::fs::metadata(asset)
            .map(|m| m.len() > 1024)
            .unwrap_or(false)
    {
        println!("cargo:rustc-cfg=has_bundled_meta_db");
    }
}
