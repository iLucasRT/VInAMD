//! What this translator's code is, as a hash of its sources: a translation
//! made by another translator must not be taken from the cache, one made by
//! the same translator in another build of the program may be
//! (`victory_gpu::shader_cache`).

use std::path::{Path, PathBuf};

fn main() {
    let mut files = Vec::new();
    collect(Path::new("src"), &mut files);
    files.push(PathBuf::from("Cargo.toml"));
    // The lock says which versions of what it is built with.
    files.push(PathBuf::from("../../Cargo.lock"));
    files.sort();
    let (mut a, mut b) = (0xcbf2_9ce4_8422_2325u64, 0x6c62_272e_07bb_0142u64);
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let Ok(bytes) = std::fs::read(file) else { continue };
        // Line ends are the checkout's, not the code's.
        let text: Vec<u8> = bytes.into_iter().filter(|&c| c != b'\r').collect();
        let name = file.to_string_lossy().replace('\\', "/");
        for &byte in name.as_bytes().iter().chain(&[0]).chain(&text).chain(&[0]) {
            a = (a ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            b = (b ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3).rotate_left(7);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=VICTORY_SHADER_SOURCES={a:016x}{b:016x}");
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
