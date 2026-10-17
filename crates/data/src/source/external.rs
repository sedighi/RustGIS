//! What the providers that run another program (GDAL's `ogr2ogr`, DuckDB) share: finding the
//! program, running it with a time limit, and the cache folder their conversions are kept in.

use crate::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A program: in the folder `env` names, else on the `PATH`.
pub(crate) fn program(env: &str, name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let dirs: Vec<PathBuf> = std::env::var_os(env)
        .map(PathBuf::from)
        .into_iter()
        .chain(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default())
        .collect();
    // Only absolute folders: an empty or relative PATH entry would look in the current folder.
    dirs.into_iter().filter(|d| d.is_absolute()).map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// A folder in the platform's cache folder (or the one `env` names, used as is).
pub(crate) fn cache_dir(env: &str, sub: &str) -> Option<PathBuf> {
    if let Some(d) = std::env::var_os(env) {
        return Some(PathBuf::from(d));
    }
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("RustGIS").join("cache"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/RustGIS"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|d| d.join("rustgis"))
    };
    base.map(|b| b.join(sub))
}

/// Runs a program, reading its output as it comes (so a chatty one can't block on a full pipe) and
/// killing it if it runs past `timeout`. `what` names it in the timeout error.
pub(crate) fn output(mut c: Command, timeout: std::time::Duration, what: &str) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let drain = |p: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = p {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{what} ran for more than {} minutes", timeout.as_secs() / 60),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Ok(std::process::Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

/// A file's size and modification time; for a folder (a File Geodatabase), those of its files,
/// so an edit inside it is noticed.
pub(crate) fn stamp(p: &Path) -> Result<(u64, u128)> {
    let m = std::fs::metadata(p)?;
    let t = |m: &std::fs::Metadata| {
        m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0)
    };
    if !m.is_dir() {
        return Ok((m.len(), t(&m)));
    }
    let (mut len, mut newest) = (0u64, t(&m));
    for e in std::fs::read_dir(p)?.flatten().take(100_000) {
        if let Ok(em) = e.metadata() {
            len = len.saturating_add(em.len());
            newest = newest.max(t(&em));
        }
    }
    Ok((len, newest))
}

/// A 64-bit FNV-1a hash: stable across Rust releases, unlike `DefaultHasher`.
pub(crate) fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Marks a cache file as used now, so pruning removes what isn't used rather than what is old.
pub(crate) fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// A file removed when dropped (unless taken out first).
pub(crate) struct TempFile(pub(crate) Option<PathBuf>);

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Removes the files in `dir` that `ours` claims and that weren't used for `keep` (best effort).
/// Nothing else the folder holds is touched.
pub(crate) fn prune(dir: &Path, ours: fn(&str) -> bool, keep: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    for e in entries.flatten().take(100_000) {
        if !e.file_name().to_str().is_some_and(ours) || !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
