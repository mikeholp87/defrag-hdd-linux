//! Read-only, descriptor-relative scanning. No symlinks or nested mounts are followed.
use linux_raw_sys::ioctl::*;
use serde::Serialize;
use std::{
    collections::HashSet,
    ffi::{CString, OsStr},
    fs::{File, Metadata, OpenOptions, ReadDir},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

const CHUNK: usize = 128;
const ENTRY_LIMIT: u64 = 100_000;
const ROW_LIMIT: usize = 5_000;
const DEPTH_LIMIT: usize = 128;

// Layout from linux/fiemap.h; reserved fields are required by the kernel ABI.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Extent {
    logical: u64,
    physical: u64,
    length: u64,
    reserved64: [u64; 2],
    flags: u32,
    reserved: [u32; 3],
}
#[repr(C)]
struct Request {
    start: u64,
    length: u64,
    flags: u32,
    mapped: u32,
    count: u32,
    reserved: u32,
    extents: [Extent; CHUNK],
}
const _: () = {
    assert!(size_of::<Extent>() == 56);
    assert!(std::mem::offset_of!(Request, extents) == 32);
};

#[derive(Debug, Default, Serialize)]
pub struct Measurement {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub allocated: u64,
    pub modified_sec: i64,
    pub modified_nsec: i64,
    pub extents: u64,
    pub mapped_bytes: u64,
    pub largest: u64,
    pub shared: u64,
    pub uncertain: u64,
    pub complete: bool,
}
impl Measurement {
    pub fn average(&self) -> u64 {
        self.mapped_bytes.checked_div(self.extents).unwrap_or(0)
    }
    fn add(&mut self, e: &Extent) -> io::Result<()> {
        self.extents = self.extents.checked_add(1).ok_or_else(overflow)?;
        self.mapped_bytes = self
            .mapped_bytes
            .checked_add(e.length)
            .ok_or_else(overflow)?;
        self.largest = self.largest.max(e.length);
        self.shared += u64::from(e.flags & FIEMAP_EXTENT_SHARED != 0);
        // Only LAST and SHARED are unambiguous for this conservative assessment.
        self.uncertain += u64::from(e.flags & !(FIEMAP_EXTENT_LAST | FIEMAP_EXTENT_SHARED) != 0);
        Ok(())
    }
}
fn overflow() -> io::Error {
    io::Error::other("Extent arithmetic overflow")
}
fn signature(m: &Metadata) -> (u64, i64, i64, i64, i64) {
    (
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn measure(file: &File, cancel: &AtomicBool) -> io::Result<Measurement> {
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::Error::other("Not a regular file"));
    }
    let mut result = Measurement {
        device: before.dev(),
        inode: before.ino(),
        size: before.len(),
        allocated: before.blocks().checked_mul(512).ok_or_else(overflow)?,
        modified_sec: before.mtime(),
        modified_nsec: before.mtime_nsec(),
        ..Default::default()
    };
    let mut start = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut request = Request {
            start,
            length: u64::MAX - start,
            flags: 0,
            mapped: 0,
            count: 128,
            reserved: 0,
            extents: [Extent::default(); CHUNK],
        };
        // SAFETY: repr(C), checked layout and initialized storage for all 128 returned extents.
        let rc = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                libc::c_ulong::from(FS_IOC_FIEMAP),
                &mut request,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        if consume(&mut result, &request, &mut start)? {
            result.complete = true;
            break;
        }
    }
    if signature(&before) != signature(&file.metadata()?) {
        result.complete = false;
    }
    Ok(result)
}

fn consume(result: &mut Measurement, request: &Request, start: &mut u64) -> io::Result<bool> {
    let count = usize::try_from(request.mapped).map_err(|_| overflow())?;
    if count > CHUNK {
        return Err(io::Error::other("Invalid kernel extent count"));
    }
    if count == 0 {
        return Ok(true);
    }
    let mut last = false;
    for e in &request.extents[..count] {
        let end = e.logical.checked_add(e.length).ok_or_else(overflow)?;
        if last || e.length == 0 || end <= *start || (*start != 0 && e.logical < *start) {
            return Err(io::Error::other(
                "Invalid or non-progressing extent response",
            ));
        }
        result.add(e)?;
        *start = end;
        last = e.flags & FIEMAP_EXTENT_LAST != 0;
    }
    Ok(last || *start == u64::MAX)
}

fn open_at(dir: i32, path: &OsStr, child: bool) -> io::Result<File> {
    let path = CString::new(path.as_bytes()).map_err(|_| io::Error::other("NUL in path"))?;
    // SAFETY: all-zero open_how is a valid initial value, including reserved fields.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = u64::try_from(libc::O_PATH | libc::O_CLOEXEC).map_err(|_| overflow())?;
    how.resolve = libc::RESOLVE_NO_SYMLINKS
        | if child {
            libc::RESOLVE_BENEATH | libc::RESOLVE_NO_XDEV
        } else {
            0
        };
    // SAFETY: valid C string, initialized ABI struct, correct size. Ownership transfers on success.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir,
            path.as_ptr(),
            &how,
            size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = i32::try_from(fd).map_err(|_| overflow())?;
    // SAFETY: syscall returned a new, uniquely owned descriptor.
    let pinned = unsafe { File::from_raw_fd(fd) };
    let metadata = pinned.metadata()?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::other("Special file excluded"));
    }
    // The validated descriptor stays alive while procfs reopens the same inode for reading.
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
}
#[derive(Debug, Serialize)]
pub struct Row {
    pub path_bytes: Vec<u8>,
    pub display_path: String,
    pub measurement: Measurement,
}
#[derive(Debug, Default, Serialize)]
pub struct Scan {
    pub examined: u64,
    pub measured: u64,
    pub complete_files: u64,
    pub skipped: u64,
    pub duplicate_links: u64,
    pub total_extents: u64,
    pub apparent_bytes: u64,
    pub allocated_bytes: u64,
    pub cancelled: bool,
    pub limited: bool,
    pub omitted_rows: u64,
    pub rows: Vec<Row>,
    pub errors: Vec<String>,
}
impl Scan {
    fn skip(&mut self, path: &Path, error: impl std::fmt::Display) {
        self.skipped += 1;
        if self.errors.len() < 100 {
            self.errors.push(format!("{}: {error}", path.display()));
        }
    }
    fn record(&mut self, path: &Path, measurement: Measurement) -> io::Result<()> {
        self.measured += 1;
        self.complete_files += u64::from(measurement.complete);
        self.total_extents = self
            .total_extents
            .checked_add(measurement.extents)
            .ok_or_else(overflow)?;
        self.apparent_bytes = self
            .apparent_bytes
            .checked_add(measurement.size)
            .ok_or_else(overflow)?;
        self.allocated_bytes = self
            .allocated_bytes
            .checked_add(measurement.allocated)
            .ok_or_else(overflow)?;
        if self.rows.len() < ROW_LIMIT {
            self.rows.push(Row {
                path_bytes: path.as_os_str().as_bytes().to_vec(),
                display_path: path.to_string_lossy().into_owned(),
                measurement,
            });
        } else {
            self.omitted_rows += 1;
        }
        Ok(())
    }
    pub fn summary(&self) -> String {
        format!(
            "{} examined · {} measured · {} complete · {} skipped · {} duplicate links\n{} reported extents · {} apparent bytes · {} allocated bytes\nCancelled: {} · Resource limit reached: {} · Detail rows omitted: {}",
            self.examined,
            self.measured,
            self.complete_files,
            self.skipped,
            self.duplicate_links,
            self.total_extents,
            self.apparent_bytes,
            self.allocated_bytes,
            self.cancelled,
            self.limited,
            self.omitted_rows
        )
    }
}
struct Directory {
    file: File,
    path: PathBuf,
    entries: ReadDir,
}
fn directory(file: File, path: PathBuf) -> io::Result<Directory> {
    let entries = std::fs::read_dir(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    Ok(Directory {
        file,
        path,
        entries,
    })
}
pub fn scan(path: &Path, cancel: &AtomicBool, progress: &AtomicU64) -> io::Result<Scan> {
    let root = open_at(libc::AT_FDCWD, path.as_os_str(), false)?;
    let meta = root.metadata()?;
    let mut result = Scan::default();
    if meta.is_file() {
        result.examined = 1;
        result.record(path, measure(&root, cancel)?)?;
        result.cancelled = cancel.load(Ordering::Relaxed);
        progress.store(1, Ordering::Relaxed);
        return Ok(result);
    }
    if !meta.is_dir() {
        return Err(io::Error::other("Select a regular file or directory"));
    }
    let mut stack = vec![directory(root, path.to_owned())?];
    let mut seen = HashSet::new();
    while let Some(parent) = stack.last_mut() {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        if result.examined >= ENTRY_LIMIT {
            result.limited = true;
            break;
        }
        let Some(entry) = parent.entries.next() else {
            stack.pop();
            continue;
        };
        result.examined += 1;
        progress.store(result.examined, Ordering::Relaxed);
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                result.skip(&parent.path, e);
                continue;
            }
        };
        let path = parent.path.join(entry.file_name());
        // Avoid opening special files; openat2 still independently rejects symlink replacements.
        match entry.file_type() {
            Ok(t) if t.is_file() || t.is_dir() => {}
            Ok(_) => {
                result.skip(&path, "Special file or symlink excluded");
                continue;
            }
            Err(e) => {
                result.skip(&path, e);
                continue;
            }
        }
        let file = match open_at(parent.file.as_raw_fd(), &entry.file_name(), true) {
            Ok(f) => f,
            Err(e) => {
                result.skip(&path, e);
                continue;
            }
        };
        let meta = match file.metadata() {
            Ok(m) => m,
            Err(e) => {
                result.skip(&path, e);
                continue;
            }
        };
        if !seen.insert((meta.dev(), meta.ino())) {
            result.duplicate_links += 1;
            continue;
        }
        if meta.is_dir() {
            if stack.len() >= DEPTH_LIMIT {
                result.limited = true;
                result.skip(&path, "Directory depth limit");
                continue;
            }
            match directory(file, path.clone()) {
                Ok(d) => stack.push(d),
                Err(e) => result.skip(&path, e),
            }
        } else if meta.is_file() {
            match measure(&file, cancel) {
                Ok(m) => result.record(&path, m)?,
                Err(e) => result.skip(&path, e),
            }
        } else {
            result.skip(&path, "Special file excluded");
        }
    }
    result.cancelled |= cancel.load(Ordering::Relaxed);
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncertain_flags_are_not_continuity() {
        let mut m = Measurement::default();
        m.add(&Extent {
            length: 4096,
            flags: FIEMAP_EXTENT_DELALLOC | FIEMAP_EXTENT_UNKNOWN,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.uncertain, 1);
        assert_eq!(m.average(), 4096);
    }
    #[test]
    fn sharing_is_separate_from_uncertainty() {
        let mut m = Measurement::default();
        m.add(&Extent {
            length: 4096,
            flags: FIEMAP_EXTENT_SHARED,
            ..Default::default()
        })
        .unwrap();
        assert_eq!((m.shared, m.uncertain), (1, 0));
    }
    #[test]
    fn overflow_is_rejected() {
        let mut m = Measurement {
            mapped_bytes: u64::MAX,
            ..Default::default()
        };
        assert!(
            m.add(&Extent {
                length: 1,
                ..Default::default()
            })
            .is_err()
        );
    }
    #[test]
    fn rejects_special_and_symlink() {
        assert!(
            scan(
                Path::new("/dev/null"),
                &AtomicBool::new(false),
                &AtomicU64::new(0)
            )
            .is_err()
        );
        assert!(
            scan(
                Path::new("/proc/self/exe"),
                &AtomicBool::new(false),
                &AtomicU64::new(0)
            )
            .is_err()
        );
    }
    #[test]
    fn pre_cancelled_scan_is_partial() {
        let s = scan(Path::new("src"), &AtomicBool::new(true), &AtomicU64::new(0)).unwrap();
        assert!(s.cancelled);
        assert_eq!(s.examined, 0);
    }
    #[test]
    fn scans_real_files_and_deduplicates_links() {
        use std::io::Write;
        let dir = tempfile::tempdir_in(".").unwrap();
        let path = dir.path().join("original");
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![42; 16_384]).unwrap();
        file.sync_all().unwrap();
        std::fs::hard_link(&path, dir.path().join("alias")).unwrap();
        std::os::unix::fs::symlink(&path, dir.path().join("link")).unwrap();
        let s = scan(dir.path(), &AtomicBool::new(false), &AtomicU64::new(0)).unwrap();
        assert_eq!((s.measured, s.duplicate_links, s.skipped), (1, 1, 1));
        assert_eq!(s.apparent_bytes, 16_384);
        assert!(s.rows[0].measurement.complete);
        assert!(s.rows[0].measurement.extents >= 1);
        assert_eq!(std::fs::read(path).unwrap(), vec![42; 16_384]);
    }
    #[test]
    fn preserves_non_utf8_paths_in_json() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let path = dir.path().join(OsStr::from_bytes(b"name-\xff"));
        File::create(&path).unwrap();
        let s = scan(dir.path(), &AtomicBool::new(false), &AtomicU64::new(0)).unwrap();
        assert_eq!(s.measured, 1);
        assert_eq!(s.rows[0].path_bytes, path.as_os_str().as_bytes());
        let json = serde_json::to_value(&s).unwrap();
        assert!(json["rows"][0]["path_bytes"].is_array());
    }
    #[test]
    fn sparse_file_is_not_counted_as_allocated_holes() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let path = dir.path().join("sparse");
        File::create(&path).unwrap().set_len(16_777_216).unwrap();
        let s = scan(&path, &AtomicBool::new(false), &AtomicU64::new(0)).unwrap();
        assert_eq!(s.apparent_bytes, 16_777_216);
        assert_eq!(s.total_extents, 0);
        assert!(s.rows[0].measurement.complete);
    }
    #[test]
    fn rejects_fifo_without_opening_for_io() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let path = dir.path().join("fifo");
        let cpath = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: valid terminated path and ordinary permission bits.
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(scan(&path, &AtomicBool::new(false), &AtomicU64::new(0)).is_err());
    }
    #[test]
    fn rejects_parent_symlinks() {
        let dir = tempfile::tempdir_in(".").unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        File::create(dir.path().join("real/file")).unwrap();
        std::os::unix::fs::symlink("real", dir.path().join("link")).unwrap();
        assert!(
            scan(
                &dir.path().join("link/file"),
                &AtomicBool::new(false),
                &AtomicU64::new(0)
            )
            .is_err()
        );
    }
    #[test]
    fn nested_mounts_are_rejected_by_kernel() {
        let root = open_at(libc::AT_FDCWD, OsStr::new("/"), false).unwrap();
        let error = open_at(root.as_raw_fd(), OsStr::new("proc"), true).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
    }
    #[test]
    fn detail_retention_is_bounded_without_losing_totals() {
        let mut s = Scan::default();
        for _ in 0..=ROW_LIMIT {
            s.record(
                Path::new("test"),
                Measurement {
                    extents: 2,
                    complete: true,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(s.rows.len(), ROW_LIMIT);
        assert_eq!(s.omitted_rows, 1);
        assert_eq!(s.total_extents, u64::try_from((ROW_LIMIT + 1) * 2).unwrap());
    }
    fn response(extents: &[Extent]) -> Request {
        let mut r = Request {
            start: 0,
            length: u64::MAX,
            flags: 0,
            mapped: u32::try_from(extents.len()).unwrap(),
            count: 128,
            reserved: 0,
            extents: [Extent::default(); CHUNK],
        };
        r.extents[..extents.len()].copy_from_slice(extents);
        r
    }
    #[test]
    fn chunk_pagination_preserves_all_metrics() {
        let first: Vec<_> = (0..128)
            .map(|i| Extent {
                logical: i * 8192,
                length: 4096,
                ..Default::default()
            })
            .collect();
        let mut m = Measurement::default();
        let mut start = 0;
        assert!(!consume(&mut m, &response(&first), &mut start).unwrap());
        let last = Extent {
            logical: 128 * 8192,
            length: 4096,
            flags: FIEMAP_EXTENT_LAST,
            ..Default::default()
        };
        assert!(consume(&mut m, &response(&[last]), &mut start).unwrap());
        assert_eq!((m.extents, m.mapped_bytes), (129, 129 * 4096));
    }
    #[test]
    fn invalid_kernel_response_is_rejected() {
        let mut r = response(&[]);
        r.mapped = 129;
        assert!(consume(&mut Measurement::default(), &r, &mut 0).is_err());
        let r = response(&[Extent {
            length: 0,
            ..Default::default()
        }]);
        assert!(consume(&mut Measurement::default(), &r, &mut 0).is_err());
        let r = response(&[Extent {
            logical: u64::MAX,
            length: 1,
            ..Default::default()
        }]);
        assert!(consume(&mut Measurement::default(), &r, &mut 0).is_err());
    }
}
