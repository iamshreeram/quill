//! Private local-file helpers.
//!
//! Unix: directories are created 0700, files written by Quill get an explicit
//! mode (0600 for secrets and history), and existing permissive files are
//! repaired. Writes never follow a symlink. Other platforms get plain fallbacks
//! (no mode bits) with the same symlink refusal.

use std::fs;
use std::io::Write;
use std::path::Path;

/// File name used in messages. Names only, never contents.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// True when `path` is a symlink, including a dangling one.
pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn note_repair(path: &Path) {
    eprintln!("[quill] repaired permissions on {}", display_name(path));
}

/// Makes sure `path` is a directory with mode 0700, creating it if missing.
/// An existing permissive directory is repaired. A symlinked directory is used
/// as-is and never chmod'ed.
pub fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if path.is_dir() {
                Ok(())
            } else {
                Err(format!(
                    "{} is a symlink to a missing target",
                    display_name(path)
                ))
            }
        }
        Ok(meta) if meta.is_dir() => repair_dir_mode(path),
        Ok(_) => Err(format!(
            "{} exists and is not a directory",
            display_name(path)
        )),
        Err(_) => create_private_dir(path),
    }
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", display_name(parent)))?;
    }
    match new_dir_builder().create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(format!("failed to create {}: {e}", display_name(path))),
    }
}

#[cfg(unix)]
fn new_dir_builder() -> fs::DirBuilder {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
}

#[cfg(not(unix))]
fn new_dir_builder() -> fs::DirBuilder {
    fs::DirBuilder::new()
}

#[cfg(unix)]
fn repair_dir_mode(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta =
        fs::metadata(path).map_err(|e| format!("failed to inspect {}: {e}", display_name(path)))?;
    if meta.permissions().mode() & 0o777 != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("failed to chmod {}: {e}", display_name(path)))?;
        note_repair(path);
    }
    Ok(())
}

#[cfg(not(unix))]
fn repair_dir_mode(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Repairs a regular, non-symlink file that has group or other permission bits
/// set: chmod 0600. Symlinks and non-regular entries are left alone. Failures
/// are ignored (best effort).
pub fn repair_file_mode(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if !meta.file_type().is_file() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0
            && fs::set_permissions(path, fs::Permissions::from_mode(0o600)).is_ok()
        {
            note_repair(path);
        }
    }
}

/// Atomically replaces `path` with `bytes`, giving the file exactly `mode`.
///
/// Refuses (Err) when `path` is a symlink, including a dangling one. The data is
/// written to a temp file in the same directory (created with create_new), then
/// renamed over the destination. The temp file is removed on failure.
pub fn write_atomic_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let name = display_name(path);
    if is_symlink(path) {
        return Err(format!("refusing to write through symlink: {name}"));
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        unique_suffix()
    ));
    match write_then_rename(&tmp, path, bytes, mode) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(format!("failed to write {name}: {e}"))
        }
    }
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn write_then_rename(tmp: &Path, dest: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let mut file = create_new_file(tmp, mode)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    set_exact_mode(tmp, mode)?;
    fs::rename(tmp, dest)
}

fn create_new_file(path: &Path, mode: u32) -> std::io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    opts.open(path)
}

#[cfg(unix)]
fn set_exact_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_exact_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Appends one line (plus a newline) to `path`, creating it with mode 0600.
///
/// Refuses (Err) when `path` is a symlink. An existing file is repaired to 0600.
pub fn append_private(path: &Path, line: &str) -> Result<(), String> {
    let name = display_name(path);
    if is_symlink(path) {
        return Err(format!("refusing to append through symlink: {name}"));
    }
    let existed = fs::symlink_metadata(path).is_ok();
    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|e| format!("failed to open {name}: {e}"))?;
    if existed {
        repair_file_mode(path);
    }
    writeln!(file, "{line}").map_err(|e| format!("failed to write {name}: {e}"))
}
