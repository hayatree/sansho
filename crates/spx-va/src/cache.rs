//! Content-addressed private cache of validated translation units and native functions.
//! Provenance: original implementation from content-addressed storage and atomic POSIX rename.

use crate::{CompiledSource, VaLoader, compile_source};
use hmac_sha256::Hash;
#[cfg(unix)]
use rustix::{
    fs::{AtFlags, Mode, OFlags, open, openat, renameat, unlinkat},
    process::geteuid,
};
use serde::{Deserialize, Serialize};
use spx_diag::Diagnostic;
use std::{cell::RefCell, fs, sync::Arc};
#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::PathBuf,
};

#[cfg(target_arch = "x86_64")]
mod sha256;

fn checksum(bytes: &[u8]) -> Digest {
    #[cfg(target_arch = "x86_64")]
    if let Some(sum) = sha256::accelerated(bytes) {
        return sum;
    }
    Hash::hash(bytes)
}

type Digest = [u8; 32];

#[derive(Debug, Clone)]
pub(crate) struct Artifact {
    dir: Arc<fs::File>,
    id: Digest,
}

fn digest(parts: &[&[u8]]) -> Digest {
    let mut hash = Hash::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        // Hash large parts independently, keeping the length and part order in the key.
        if part.len() >= 1024 {
            hash.update(checksum(part));
        } else {
            hash.update(part);
        }
    }
    hash.finalize()
}

impl Artifact {
    pub(crate) fn child(&self, parts: &[&[u8]]) -> Self {
        let mut hash = Hash::new();
        hash.update(self.id);
        hash.update(digest(parts));
        Self {
            dir: self.dir.clone(),
            id: hash.finalize(),
        }
    }

    #[cfg(unix)]
    fn name(&self) -> String {
        use std::fmt::Write;
        let mut name = String::with_capacity(64);
        for b in self.id {
            let _ = write!(name, "{b:02x}");
        }
        name
    }

    #[cfg(unix)]
    fn root(path: &str, text: &str) -> Option<Self> {
        if std::env::var_os("SPX_CACHE").is_some_and(|v| v == "0") {
            return None;
        }
        let dir = std::env::var_os("SPX_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_CACHE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
                    .map(|p| p.join("spx"))
            })?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .ok()?;
        // Pin the checked directory. Replacing an ancestor or symlink cannot
        // redirect subsequent opens, unlinks or renames into another directory.
        let dir = fs::File::from(
            open(
                &dir,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .ok()?,
        );
        let meta = dir.metadata().ok()?;
        if meta.uid() != geteuid().as_raw() || meta.mode() & 0o022 != 0 {
            return None;
        }
        let id = digest(&[
            env!("SPX_VA_BUILD").as_bytes(),
            b"manifest",
            path.as_bytes(),
            text.as_bytes(),
        ]);
        Some(Self {
            dir: dir.into(),
            id,
        })
    }

    #[cfg(not(unix))]
    fn root(_: &str, _: &str) -> Option<Self> {
        None
    }

    #[cfg(unix)]
    pub(crate) fn read(&self) -> Option<Vec<u8>> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let mut file = fs::File::from(openat(&*self.dir, self.name(), flags, Mode::empty()).ok()?);
        let meta = file.metadata().ok()?;
        if meta.uid() != geteuid().as_raw() || meta.mode() & 0o022 != 0 {
            return None;
        }
        if !meta.is_file() || !(32..=(128 << 20)).contains(&meta.len()) {
            return None;
        }
        let mut sum = [0; 32];
        file.read_exact(&mut sum).ok()?;
        let mut payload = Vec::with_capacity(meta.len() as usize - 32);
        file.take(meta.len() - 32).read_to_end(&mut payload).ok()?;
        (digest(&[&self.id, &payload]) == sum).then_some(payload)
    }

    #[cfg(unix)]
    pub(crate) fn write(&self, bytes: &[u8]) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = format!(
            ".{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC;
        if let Ok(fd) = openat(&*self.dir, &tmp, flags, Mode::RUSR | Mode::WUSR) {
            let mut file = fs::File::from(fd);
            if file
                .write_all(&digest(&[&self.id, bytes]))
                .and_then(|()| file.write_all(bytes))
                .is_ok()
            {
                let _ = renameat(&*self.dir, &tmp, &*self.dir, self.name());
            }
            let _ = unlinkat(&*self.dir, tmp, AtFlags::empty());
        }
    }
    #[cfg(not(unix))]
    pub(crate) fn read(&self) -> Option<Vec<u8>> {
        None
    }
    #[cfg(not(unix))]
    pub(crate) fn write(&self, _: &[u8]) {}
}

// Replay include lookups, including unsuccessful overrides of standard headers.
// The source digest comes from exactly the bytes delivered to the preprocessor.
#[derive(Deserialize, Serialize)]
struct Include {
    path: String,
    from: Option<String>,
    standard: bool,
    resolved: Option<(String, Digest)>,
}

struct Recording<'a> {
    loader: &'a dyn VaLoader,
    includes: RefCell<Vec<Include>>,
}

impl Recording<'_> {
    fn record(
        &self,
        path: &str,
        from: Option<&str>,
        standard: bool,
        value: Option<&(String, String)>,
    ) {
        self.includes.borrow_mut().push(Include {
            path: path.into(),
            from: from.map(String::from),
            standard,
            resolved: value.map(|(name, text)| (name.clone(), checksum(text.as_bytes()))),
        });
    }
}

impl VaLoader for Recording<'_> {
    fn load(&self, path: &str, from: Option<&str>) -> Result<(String, String), String> {
        let pair = self.loader.load(path, from)?;
        self.record(path, from, false, Some(&pair));
        Ok(pair)
    }

    fn standard_header(
        &self,
        path: &str,
        from: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        let value = self.loader.standard_header(path, from)?;
        self.record(path, from, true, value.as_ref());
        Ok(value)
    }
}

impl Include {
    fn matches(&self, loader: &dyn VaLoader) -> bool {
        let from = self.from.as_deref();
        let result = if self.standard {
            loader.standard_header(&self.path, from)
        } else {
            loader.load(&self.path, from).map(Some)
        };
        match (result, &self.resolved) {
            (Ok(Some((path, text))), Some((name, sum))) => {
                &path == name && checksum(text.as_bytes()) == *sum
            }
            (Ok(None), None) => true,
            _ => false,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Manifest {
    includes: Vec<Include>,
    sources: Vec<String>,
    declarations: Vec<(spx_diag::Span, String)>,
}

impl Manifest {
    fn restore(
        root: &Artifact,
        loader: &dyn VaLoader,
        wanted: &impl Fn(&str) -> bool,
    ) -> Option<CompiledSource> {
        let bytes = root.read()?;
        let manifest: Self = serde_json::from_slice(&bytes).ok()?;
        if !manifest.includes.iter().all(|i| i.matches(loader)) {
            return None;
        }
        let unit = root.child(&[b"typed", &bytes]);
        let mut modules = Vec::new();
        for (idx, (_, name)) in manifest.declarations.iter().enumerate() {
            if wanted(name) {
                let entry = unit.child(&[&idx.to_le_bytes()]);
                let payload = entry.read()?;
                let mut decoder = serde_json::Deserializer::from_slice(&payload);
                let mut module = crate::ir::CompiledModuleImage::deserialize(&mut decoder).ok()?;
                decoder.end().ok()?;
                module.artifact = Some(entry);
                modules.push(module);
            }
        }
        Some(CompiledSource {
            sources: manifest.sources,
            declarations: manifest.declarations,
            modules,
        })
    }
}

/// Compile every module on a miss. Restore only required bodies from a checked unit on a hit.
pub fn compile_source_cached(
    path: &str,
    text: &str,
    loader: &dyn VaLoader,
    wanted: impl Fn(&str) -> bool,
) -> Result<CompiledSource, (Vec<Diagnostic>, Vec<String>)> {
    let Some(root) = Artifact::root(path, text) else {
        return compile_source(text, loader);
    };
    let read = spx_diag::Phase::start("va_cache_read");
    if let Some(hit) = Manifest::restore(&root, loader, &wanted) {
        return Ok(hit);
    }
    drop(read);
    let recording = Recording {
        loader,
        includes: Default::default(),
    };
    let mut source = compile_source(text, &recording)?;
    let _write = spx_diag::Phase::start("va_cache_write");
    let manifest = Manifest {
        includes: recording.includes.into_inner(),
        sources: source.sources.clone(),
        declarations: source.declarations.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec(&manifest) {
        let unit = root.child(&[b"typed", &bytes]);
        for (idx, module) in source.modules.iter_mut().enumerate() {
            let entry = unit.child(&[&idx.to_le_bytes()]);
            if let Ok(payload) = serde_json::to_vec(module) {
                entry.write(&payload);
            }
            module.artifact = Some(entry);
        }
        root.write(&bytes);
    }
    source.modules.retain(|m| wanted(&m.name));
    Ok(source)
}

pub(crate) mod bits {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(value.to_bits())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        u64::deserialize(d).map(f64::from_bits)
    }
}

pub(crate) mod vector_bits {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(values: &[f64], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(values.iter().map(|v| v.to_bits()))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        Vec::<u64>::deserialize(d).map(|bits| bits.into_iter().map(f64::from_bits).collect())
    }
}
