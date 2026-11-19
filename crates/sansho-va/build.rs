//! Version the private VA artifact format and compiler implementation together.
//! Provenance: original build fingerprint using the Rust standard library.

use std::{
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    path::Path,
};

fn fingerprint(path: &Path, hash: &mut DefaultHasher) -> io::Result<()> {
    if path.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            fingerprint(&entry.path(), hash)?;
        }
    } else {
        path.hash(hash);
        fs::read(path)?.hash(hash);
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mut hash = DefaultHasher::new();
    for path in ["src", "build.rs", "Cargo.toml", "../../Cargo.lock"] {
        println!("cargo:rerun-if-changed={path}");
        fingerprint(Path::new(path), &mut hash)?;
    }
    let compiler =
        std::process::Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg("-vV")
            .output()?;
    compiler.stdout.hash(&mut hash);
    for key in [
        "TARGET",
        "PROFILE",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_PKG_VERSION",
    ] {
        env::var_os(key).hash(&mut hash);
    }
    println!("cargo:rustc-env=SANSHO_VA_BUILD={:016x}", hash.finish());
    Ok(())
}
