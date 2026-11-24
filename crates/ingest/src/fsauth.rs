// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj

//! Filesystem authority: where a file butterfly opened really is, and
//! whether a path it was handed may be read at all.
//!
//! Paths reach butterfly from untrusted documents -- a coverage export names
//! the sources, an evidence file names them again for the viewer's freshness
//! check. Every such read goes through [`open_placed`]: the name is opened
//! once, and the kernel is then asked where that descriptor points, so the
//! object whose place is checked is the object the bytes come from (#1,
//! review of #21).
//!
//! A place is not a name, though. A hard-linked file has several names and
//! the kernel reports only one of them -- Linux the one it was opened by,
//! macOS whichever `F_GETPATH` settles on. An in-root name for an inode that
//! also lives outside the root therefore passed the check, and macOS lets
//! anyone hard-link a file they cannot read (#248). No kernel keeps a list of
//! an inode's names, so a regular file with more than one cannot be placed,
//! and is reported as such rather than guessed about.

use anyhow::Result;
use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};

// Containment is enforced by asking the kernel where an already-open file is,
// which every supported platform answers differently and none answers
// portably. A target without an implementation must fail to build rather than
// quietly fall back to resolving the name a second time.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("butterfly needs a way to ask the kernel where an open file is; see path_of in crates/ingest/src/fsauth.rs");

/// Where an open file actually is, according to the kernel rather than to the
/// name it was opened by.
///
/// Canonicalising a name and then reading that name back is two separate
/// lookups, and between them an attacker who can write in the tree can
/// re-aim the file, or any directory above it, at something outside the
/// root. Asking the descriptor removes the second lookup.
#[cfg(target_os = "linux")]
pub fn path_of(f: &File) -> Result<PathBuf> {
    use anyhow::Context;
    use std::os::fd::AsRawFd;
    let link = format!("/proc/self/fd/{}", f.as_raw_fd());
    std::fs::read_link(&link).with_context(|| format!("asking {link} where an open file is"))
}

/// The same question, asked the way macOS answers it.
#[cfg(target_os = "macos")]
pub fn path_of(f: &File) -> Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // F_GETPATH writes the path of the open file into a PATH_MAX buffer
    if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
        anyhow::bail!("asking the kernel where an open file is: {}", std::io::Error::last_os_error());
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(OsStr::from_bytes(&buf[..end])))
}

/// Where an opened file stands relative to the root it was meant to be in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Its only name is inside the root.
    Inside,
    /// The kernel places it outside the root.
    Outside,
    /// A regular file with this many names. Where the others are cannot be
    /// found out, so whether it is inside the root cannot be established.
    Linked(u64),
}

/// A file opened once, with what the kernel says about that descriptor.
/// Nothing in it has been read yet.
#[derive(Debug)]
pub struct Placed {
    pub file: File,
    /// Where the kernel says it is.
    pub real: PathBuf,
    pub meta: Metadata,
    pub standing: Standing,
}

/// Open `name` once and place it relative to `root`, which must already be
/// canonical (the caller resolves the operator's root once, not per file).
///
/// Opened `O_NONBLOCK`, so a fifo planted among the files cannot hang the
/// open; the caller rejects anything that is not a regular file. The only
/// `Err` is an open or `fstat` that failed, or a kernel that would not say
/// where the file is -- a refusal to place it is a [`Standing`], not an
/// error, so the caller can apply an operator's override to it.
pub fn open_placed(root: &Path, name: &Path) -> std::io::Result<Placed> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(name)?;
    let real = path_of(&file).map_err(|e| std::io::Error::other(e.to_string()))?;
    let meta = file.metadata()?;
    let standing = if !real.starts_with(root) {
        Standing::Outside
    } else if meta.is_file() && meta.nlink() > 1 {
        Standing::Linked(meta.nlink())
    } else {
        Standing::Inside
    };
    Ok(Placed { file, real, meta, standing })
}

/// Why a file refused as [`Standing::Linked`] was refused, shared so the
/// ingest error and the viewer's freshness reason say the same thing.
pub const LINKED_REASON: &str = "the kernel reports only one of a file's names, so whether every name for it is inside the root cannot be established";

/// Open the file at `path` itself, refusing a symlink there (#341).
///
/// For a file butterfly found by its name in a directory rather than one the
/// operator named: a link planted under that name would otherwise hand the
/// read -- and, for `build-info.txt`, the evidence -- whatever it points at.
/// `O_NOFOLLOW` makes the refusal part of the open, so there is no window
/// between a check and the read in which the name could be re-aimed; only
/// the final component is held to it, because the directory above is the
/// one the operator chose. `O_NONBLOCK`, as in [`open_placed`], so a fifo
/// cannot hang the open; the caller's `fstat` rejects anything that is not a
/// regular file.
pub fn open_nofollow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path).map_err(|e| {
        // ELOOP is how both kernels say "a symlink, and you asked not to follow it"
        if e.raw_os_error() == Some(libc::ELOOP) {
            std::io::Error::other("it is a symlink")
        } else {
            e
        }
    })
}

/// Open a file discovered by name in a build directory, refusing both symlinks
/// (#341) and hard-linked regular files (#248).
///
/// On macOS any user who can write to a shared build directory can hard-link a
/// file they cannot read into it (`ln ~/.aws/credentials build-info.txt`);
/// `O_NOFOLLOW` alone does not catch that. Checking `fstat`'s `nlink` on the
/// already-open descriptor closes the window without a second path lookup.
pub fn open_discovered(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::MetadataExt;
    let file = open_nofollow(path)?;
    let meta = file.metadata()?;
    if meta.is_file() && meta.nlink() > 1 {
        return Err(std::io::Error::other(format!("it has {} hard links ({LINKED_REASON})", meta.nlink())));
    }
    Ok(file)
}

// ---- a directory the build owns -----------------------------------------
//
// `butterfly build` cleans and refills `out/<variant>` and `out/coverage`.
// Checking once that such a directory is a real one and then addressing what
// is in it by path resolves the directory again at every step, so a swap of
// `out/<variant>` for a symlink after the check redirected the clean into the
// link's target (#343). A [`HeldDir`] is the checked directory, held open:
// everything the build removes or recreates in it is done relative to that
// descriptor (`unlinkat`, `mkdirat`, `openat` with `O_NOFOLLOW`), as
// [`write_atomic`] does for a file, so no step can be redirected after the
// check.

/// What is at a name in a [`HeldDir`], asked without following a symlink.
#[derive(Debug, PartialEq, Eq)]
pub enum Entry {
    Absent,
    /// A symlink, with where it points.
    Symlink(PathBuf),
    Dir,
    /// Anything else: a regular file, a fifo, a socket.
    Other,
}

/// A directory opened once, by a name whose final component is not a
/// symlink, and used from then on only through the descriptor.
#[derive(Debug)]
pub struct HeldDir {
    file: File,
    path: PathBuf,
}

/// Open the directory `name` in `dir` -- or at `path` when `dir` is `None` --
/// refusing a symlink there.
fn open_dir_nofollow(dir: Option<&File>, name: &Path) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let c = cstr(name.as_os_str())?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let at = dir.map_or(libc::AT_FDCWD, |d| d.as_raw_fd());
    let fd = unsafe { libc::openat(at, c.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// The names in the directory open as `dir`, without `.` and `..`.
fn names_in(dir: &File) -> std::io::Result<Vec<std::ffi::OsString>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    // fdopendir takes the descriptor over, so it gets a copy of its own
    let fd = cvt(unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
    let d = unsafe { libc::fdopendir(fd) };
    if d.is_null() {
        let e = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    // a descriptor shares its offset with its copies: start from the top
    unsafe { libc::rewinddir(d) };
    let mut out = Vec::new();
    loop {
        let ent = unsafe { libc::readdir(d) };
        if ent.is_null() {
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            out.push(std::ffi::OsStr::from_bytes(name.to_bytes()).to_os_string());
        }
    }
    unsafe { libc::closedir(d) };
    Ok(out)
}

/// Remove the directory `name` in `dir` and everything under it, through
/// descriptors all the way down: each level is opened `O_NOFOLLOW` relative
/// to its parent, and a symlink anywhere in the tree is unlinked as itself.
fn remove_tree_at(dir: &File, name: &std::ffi::OsStr) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let sub = open_dir_nofollow(Some(dir), Path::new(name))?;
    for n in names_in(&sub)? {
        let c = cstr(&n)?;
        match stat_at(&sub, &c) {
            Some(st) if (st.st_mode & libc::S_IFMT) == libc::S_IFDIR => remove_tree_at(&sub, &n)?,
            Some(_) => {
                cvt(unsafe { libc::unlinkat(sub.as_raw_fd(), c.as_ptr(), 0) })?;
            }
            None => {}
        }
    }
    let c = cstr(name)?;
    cvt(unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), libc::AT_REMOVEDIR) })?;
    Ok(())
}

impl HeldDir {
    /// Open the directory at `path`, refusing a symlink as its final
    /// component. The directories above it are resolved once, now: they are
    /// the ones the operator chose.
    pub fn open(path: &Path) -> std::io::Result<HeldDir> {
        Ok(HeldDir { file: open_dir_nofollow(None, path)?, path: path.to_path_buf() })
    }

    /// The name it was opened by -- for messages, and for the commands that
    /// can only be given a path. What the build itself removes or recreates
    /// goes through the descriptor, never through this.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What is at `name` here, without following a symlink.
    pub fn entry(&self, name: &std::ffi::OsStr) -> std::io::Result<Entry> {
        let c = cstr(name)?;
        Ok(match stat_at(&self.file, &c) {
            None => Entry::Absent,
            Some(st) if is_link(&st) => Entry::Symlink(read_link_at(&self.file, &c).unwrap_or_default()),
            Some(st) if (st.st_mode & libc::S_IFMT) == libc::S_IFDIR => Entry::Dir,
            Some(_) => Entry::Other,
        })
    }

    /// The names here, without `.` and `..`.
    pub fn names(&self) -> std::io::Result<Vec<std::ffi::OsString>> {
        names_in(&self.file)
    }

    /// Unlink `name` here: a file, or a symlink as itself. A directory is
    /// refused by the kernel.
    pub fn unlink(&self, name: &std::ffi::OsStr) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let c = cstr(name)?;
        cvt(unsafe { libc::unlinkat(self.file.as_raw_fd(), c.as_ptr(), 0) })?;
        Ok(())
    }

    /// Remove the directory `name` here and everything in it, never leaving
    /// this directory: a symlink in the tree is removed, not followed.
    pub fn remove_tree(&self, name: &std::ffi::OsStr) -> std::io::Result<()> {
        remove_tree_at(&self.file, name)
    }

    /// An empty directory at `name` here, whatever was there, held open in
    /// turn.
    pub fn fresh_subdir(&self, name: &std::ffi::OsStr) -> std::io::Result<HeldDir> {
        use std::os::fd::AsRawFd;
        match self.entry(name)? {
            Entry::Absent => {}
            Entry::Dir => self.remove_tree(name)?,
            Entry::Symlink(_) | Entry::Other => self.unlink(name)?,
        }
        let c = cstr(name)?;
        cvt(unsafe { libc::mkdirat(self.file.as_raw_fd(), c.as_ptr(), 0o777) })?;
        Ok(HeldDir { file: open_dir_nofollow(Some(&self.file), Path::new(name))?, path: self.path.join(name) })
    }
}

// ---- replacing an output ------------------------------------------------
//
// An output is written to a uniquely named temporary file beside the
// destination, flushed to disk, and renamed over the top. The rename replaces
// the directory entry itself, which is what makes it both atomic -- a reader
// sees the old document or the new one, never a partial one -- and
// symlink-proof: a link at the destination is replaced rather than written
// through (#2).
//
// The temporary file is created `O_CREAT|O_EXCL|O_NOFOLLOW`: a name planted
// as a link to something butterfly may write makes the create fail, never
// truncates the target (review of #22).
//
// Every step is relative to one descriptor for the destination's directory,
// opened once at the start. Doing them by path resolved the directory again
// at each step, so a directory swapped for a link to somewhere else between
// the create and the rename moved the write with it (#248). The directory
// the operator named is resolved exactly once -- through any symlinks in it,
// because that is the directory they chose (macOS's own /tmp and /var are
// links) -- and nothing after that is looked up by name outside it.

/// What the write had to do to the destination, for the caller to report.
#[derive(Debug, PartialEq, Eq)]
pub enum Wrote {
    /// A regular file (or nothing) was there.
    Normally,
    /// The destination was a symlink. It was replaced, not written through;
    /// the target it pointed at is untouched.
    ReplacedSymlink(PathBuf),
}

fn cstr(name: &std::ffi::OsStr) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(name.as_bytes()).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a file name with a NUL byte in it"))
}

/// Check a libc return value, turning -1 into the OS error.
fn cvt(r: libc::c_int) -> std::io::Result<libc::c_int> {
    if r == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// What is at `name` in `dir`, asked without following a final symlink.
fn stat_at(dir: &File, name: &std::ffi::CStr) -> Option<libc::stat> {
    use std::os::fd::AsRawFd;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstatat(dir.as_raw_fd(), name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0).then_some(st)
}

/// The mode a new output takes over from what is already at its name, if any.
///
/// An existing output's permissions are the operator's, not ours -- but only
/// a real file's, and only one the operator owns. Through a symlink the mode
/// would be that of whatever it points at; a file someone else planted here
/// carries their choice of mode, and a world-writable one would let them edit
/// the output after it is written (#336). In either case the new file keeps
/// the mode it was created with. The setuid, setgid and sticky bits are never
/// carried over.
fn inherited_mode(st: &libc::stat, euid: libc::uid_t) -> Option<libc::mode_t> {
    ((st.st_mode & libc::S_IFMT) == libc::S_IFREG && st.st_uid == euid).then_some(st.st_mode & 0o777)
}

fn is_link(st: &libc::stat) -> bool {
    (st.st_mode & libc::S_IFMT) == libc::S_IFLNK
}

fn read_link_at(dir: &File, name: &std::ffi::CStr) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    let n = unsafe { libc::readlinkat(dir.as_raw_fd(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    (n >= 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..n as usize])))
}

/// A name beside the destination that an onlooker cannot predict.
///
/// Unpredictability is not what makes the write safe -- the exclusive create
/// is -- but a guessable name lets anyone who can write in the directory
/// occupy every candidate and keep the output from ever being saved. The
/// digest is over the things that separate one attempt from another: the
/// process, a counter within it, the clock, and the address of a stack
/// local, which the allocator and ASLR both move.
fn temp_name(name: &std::ffi::OsStr) -> std::ffi::OsString {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);

    let here = 0u8;
    let mut h = Sha256::new();
    h.update(std::process::id().to_le_bytes());
    h.update(N.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    h.update(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).to_le_bytes());
    h.update((std::ptr::addr_of!(here) as usize).to_le_bytes());
    h.update(name.as_bytes());
    let tag = h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect::<String>();
    let mut t = std::ffi::OsString::from(".");
    t.push(name);
    t.push(format!(".tmp-{tag}"));
    t
}

/// Create the temporary file in `dir`, or fail -- never open something
/// already there. The retry only covers the vanishingly unlikely honest
/// collision; a directory an attacker floods eventually reports that it
/// could not create the file, which is the correct outcome -- the
/// destination is left as it was.
fn create_exclusive_at(dir: &File, name: &std::ffi::OsStr) -> std::io::Result<(std::ffi::CString, File)> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut last = None;
    for _ in 0..32 {
        let tmp = cstr(&temp_name(name))?;
        // The mode `File::create` would have given, masked by umask as
        // open(2) does, so a first output is as readable as it always was.
        // An existing destination's mode is copied over before any byte is
        // written.
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let fd = unsafe { libc::openat(dir.as_raw_fd(), tmp.as_ptr(), flags, 0o666 as libc::c_uint) };
        if fd >= 0 {
            return Ok((tmp, unsafe { File::from_raw_fd(fd) }));
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(e);
        }
        last = Some(e);
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free temporary name")))
}

/// Write `bytes` to `dest`, atomically, without following a symlink there
/// and without resolving its directory more than once.
///
/// On any failure the destination is left exactly as it was and the
/// temporary file is removed, so the last complete document survives.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<Wrote> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let name = dest.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} does not name a file", dest.display())))?;
    let dir_path = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let dir = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC).open(dir_path)?;
    let cname = cstr(name)?;

    let existing = stat_at(&dir, &cname);
    let link = existing.as_ref().filter(|st| is_link(st)).map(|_| read_link_at(&dir, &cname).unwrap_or_default());
    let (tmp, mut f) = create_exclusive_at(&dir, name)?;
    let mut write = || -> std::io::Result<()> {
        if let Some(mode) = existing.as_ref().and_then(|st| inherited_mode(st, unsafe { libc::geteuid() })) {
            cvt(unsafe { libc::fchmod(f.as_raw_fd(), mode) })?;
        }
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
        cvt(unsafe { libc::renameat(dir.as_raw_fd(), tmp.as_ptr(), dir.as_raw_fd(), cname.as_ptr()) })?;
        Ok(())
    };
    match write() {
        Ok(()) => {
            // the rename itself is only durable once the directory entry is
            let _ = dir.sync_all();
            Ok(match link {
                Some(target) => Wrote::ReplacedSymlink(target),
                None => Wrote::Normally,
            })
        }
        Err(e) => {
            unsafe { libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0) };
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        butterfly_testsupport::scratch_dir(&format!("fsauth-{tag}"))
    }

    fn stat_of(kind: libc::mode_t, perm: libc::mode_t, uid: libc::uid_t) -> libc::stat {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_mode = kind | perm;
        st.st_uid = uid;
        st
    }

    /// A planted file's mode is not taken over, and the special bits never
    /// are (#336). A file owned by someone else cannot be made here without
    /// root, so the decision is checked directly.
    #[test]
    fn only_the_operators_own_file_lends_its_mode_and_never_its_special_bits() {
        let me: libc::uid_t = 501;
        assert_eq!(inherited_mode(&stat_of(libc::S_IFREG, 0o640, me), me), Some(0o640), "the operator's own permissions stand");
        assert_eq!(inherited_mode(&stat_of(libc::S_IFREG, 0o6755, me), me), Some(0o755), "setuid and setgid are dropped");
        assert_eq!(inherited_mode(&stat_of(libc::S_IFREG, 0o1777, me), me), Some(0o777), "the sticky bit is dropped");
        assert_eq!(inherited_mode(&stat_of(libc::S_IFREG, 0o666, me + 1), me), None, "a file someone else planted lends nothing");
        assert_eq!(inherited_mode(&stat_of(libc::S_IFLNK, 0o777, me), me), None, "nor does a symlink");
        assert_eq!(inherited_mode(&stat_of(libc::S_IFDIR, 0o777, me), me), None, "nor a directory");
    }

    #[test]
    fn rewriting_an_own_output_keeps_its_permissions_but_not_setuid() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("mode");
        let dest = d.join("evidence.json");
        fs::write(&dest, b"old").unwrap();
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&dest, b"new").unwrap();
        assert_eq!(fs::metadata(&dest).unwrap().permissions().mode() & 0o7777, 0o640);
        assert_eq!(fs::read(&dest).unwrap(), b"new");

        fs::set_permissions(&dest, fs::Permissions::from_mode(0o4644)).unwrap();
        write_atomic(&dest, b"newer").unwrap();
        assert_eq!(fs::metadata(&dest).unwrap().permissions().mode() & 0o7777, 0o644, "setuid is not carried onto the new output");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn the_temporary_name_cannot_be_guessed_from_the_destination() {
        let d = scratch("tmpname");
        let dir = File::open(&d).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let (tmp, f) = create_exclusive_at(&dir, std::ffi::OsStr::new("resolutions.json")).unwrap();
            drop(f);
            let name = tmp.to_string_lossy().into_owned();
            assert!(seen.insert(name.clone()), "the same temporary name came up twice: {name}");
            assert!(!name.contains(&format!("tmp-{}-", std::process::id())), "the name still spells out the process id and a counter: {name}");
            fs::remove_file(d.join(&name)).unwrap();
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// The exclusive create is the guarantee: anything already at the chosen
    /// name -- a symlink most of all -- makes the create fail rather than
    /// truncate what it points at. Occupy the name and the writer moves on.
    #[test]
    fn an_occupied_name_is_never_opened() {
        let d = scratch("occupied");
        let dir = File::open(&d).unwrap();
        let victim = d.join("victim.json");
        fs::write(&victim, "PRECIOUS").unwrap();
        let squatted = temp_name(std::ffi::OsStr::new("resolutions.json"));
        std::os::unix::fs::symlink(&victim, d.join(&squatted)).unwrap();

        let (tmp, f) = create_exclusive_at(&dir, std::ffi::OsStr::new("resolutions.json")).unwrap();
        drop(f);
        assert_ne!(tmp.to_string_lossy(), squatted.to_string_lossy(), "the occupied name was not reused");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "PRECIOUS", "and nothing was written through it");
        let _ = fs::remove_dir_all(&d);
    }

    /// The directory is bound once. Swapping it for a link to somewhere else
    /// after the descriptor is open does not move the write: it lands in the
    /// directory the operator named, and the other place is untouched.
    #[test]
    fn swapping_the_directory_after_it_is_bound_does_not_move_the_write() {
        let d = scratch("swap");
        let out = d.join("out");
        let elsewhere = d.join("elsewhere");
        fs::create_dir_all(&out).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        let dir = File::open(&out).unwrap();
        // the operator's directory moves away and a link takes its name
        fs::rename(&out, d.join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &out).unwrap();

        let (tmp, f) = create_exclusive_at(&dir, std::ffi::OsStr::new("cache.json")).unwrap();
        drop(f);
        assert!(d.join("moved").join(tmp.to_str().unwrap()).exists(), "the file was created in the bound directory");
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "and nothing appeared where the link points");
        let _ = fs::remove_dir_all(&d);
    }

    /// A held directory is cleaned through its descriptor, all the way down:
    /// a link in the tree is removed as itself, and the directory moved away
    /// and replaced by a link after it was opened is still the one cleaned
    /// (#343).
    #[test]
    fn a_held_directory_is_cleaned_through_its_descriptor_never_through_a_link() {
        use std::ffi::OsStr;
        let d = scratch("held");
        let outside = d.join("outside");
        fs::create_dir_all(outside.join("obj")).unwrap();
        fs::write(outside.join("obj/precious"), "keep").unwrap();
        let v = d.join("v");
        fs::create_dir_all(v.join("obj/deep/er")).unwrap();
        fs::write(v.join("obj/deep/er/old.o"), "old").unwrap();
        std::os::unix::fs::symlink(&outside, v.join("obj/deep/link")).unwrap();

        // a symlink as the final component is refused outright
        std::os::unix::fs::symlink(&outside, d.join("alias")).unwrap();
        assert!(HeldDir::open(&d.join("alias")).is_err(), "a link is not held as a directory");

        let held = HeldDir::open(&v).unwrap();
        fs::rename(&v, d.join("moved")).unwrap();
        std::os::unix::fs::symlink(&outside, &v).unwrap();
        assert_eq!(held.entry(OsStr::new("obj")).unwrap(), Entry::Dir);
        let obj = held.fresh_subdir(OsStr::new("obj")).unwrap();
        assert_eq!(fs::read_to_string(outside.join("obj/precious")).unwrap(), "keep", "neither the swapped-in link nor the one in the tree was followed");
        assert!(fs::read_dir(d.join("moved/obj")).unwrap().next().is_none(), "the held directory's obj is empty and new");
        assert_eq!(obj.path(), v.join("obj"));

        fs::write(d.join("moved/stale.su"), "x").unwrap();
        assert!(held.names().unwrap().iter().any(|n| n == "stale.su"));
        held.unlink(OsStr::new("stale.su")).unwrap();
        assert_eq!(held.entry(OsStr::new("stale.su")).unwrap(), Entry::Absent);
        assert!(held.unlink(OsStr::new("obj")).is_err(), "unlink never removes a directory");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn open_discovered_refuses_symlinks_and_hardlinks() {
        let d = scratch("open-disc");
        let reg = d.join("regular.txt");
        fs::write(&reg, b"ok").unwrap();
        assert!(open_discovered(&reg).is_ok(), "single-name regular file is allowed");

        let sym = d.join("sym.txt");
        std::os::unix::fs::symlink(&reg, &sym).unwrap();
        let sym_err = open_discovered(&sym).unwrap_err().to_string();
        assert!(sym_err.contains("symlink"), "{sym_err}");

        let hl = d.join("hard.txt");
        fs::hard_link(&reg, &hl).unwrap();
        let hl_err = open_discovered(&hl).unwrap_err().to_string();
        assert!(hl_err.contains("hard link"), "{hl_err}");
        let _ = fs::remove_dir_all(d);
    }
}
