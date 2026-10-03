use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=LICENSE");
    println!("cargo:rerun-if-changed=assets/ffrm.ico");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    embed_icon(&manifest);
    let mut destination = manifest.join("target");
    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    if !target.is_empty() && target != host {
        destination.push(target);
    }
    destination.push(env::var("PROFILE").unwrap_or_else(|_| "debug".to_string()));
    if fs::create_dir_all(&destination).is_ok() {
        let _ = fs::copy(manifest.join("LICENSE"), destination.join("LICENSE"));
    }
}

fn embed_icon(manifest: &Path) {
    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
        return;
    }
    let icon = manifest.join("assets").join("ffrm.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon.to_str().expect("图标路径"));
    resource.compile().expect("无法嵌入应用图标");
}
