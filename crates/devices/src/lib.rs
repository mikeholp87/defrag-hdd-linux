//! Read-only mount discovery from /proc/self/mountinfo. Mount paths stay raw bytes.
pub mod topology;

use std::{
    ffi::{CString, OsStr},
    fs, io,
    os::unix::ffi::OsStrExt,
    path::Path,
};

/// What a filesystem type is, independent of any maintenance backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Kernel/virtual or memory-backed: never a defragmentation target.
    Pseudo,
    /// Remote: not statted (a dead server can hang the caller) and not a target.
    Network,
    Real,
}

/// Whether a maintenance backend is planned for the filesystem.
/// `Unsupported` = recognised, no backend. `Unknown` = unrecognised type; never guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Ext4,
    Xfs,
    Btrfs,
    Unsupported,
    Unknown,
}

pub fn classify(fstype: &str) -> Class {
    match fstype {
        "sysfs" | "proc" | "devtmpfs" | "devpts" | "tmpfs" | "ramfs" | "cgroup" | "cgroup2"
        | "securityfs" | "efivarfs" | "debugfs" | "tracefs" | "configfs" | "fusectl" | "pstore"
        | "bpf" | "mqueue" | "hugetlbfs" | "autofs" | "binfmt_misc" | "selinuxfs" | "nsfs"
        | "rpc_pipefs" | "squashfs" | "overlay" => Class::Pseudo,
        "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "ceph" | "glusterfs" | "afs" => Class::Network,
        t if t.starts_with("fuse") => Class::Network,
        _ => Class::Real,
    }
}

pub fn support(fstype: &str) -> Support {
    match fstype {
        "ext4" => Support::Ext4,
        "xfs" => Support::Xfs,
        "btrfs" => Support::Btrfs,
        "ext2" | "ext3" | "vfat" | "exfat" | "ntfs" | "ntfs3" | "f2fs" | "zfs" | "bcachefs"
        | "iso9660" | "udf" | "jfs" | "reiserfs" => Support::Unsupported,
        _ => Support::Unknown,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Mount {
    pub id: u32,
    pub parent_id: u32,
    pub major: u32,
    pub minor: u32,
    /// Path within the filesystem that is mounted here (differs for bind/subvolume mounts).
    pub root: Vec<u8>,
    /// Raw mount point bytes; may be non-UTF-8.
    pub target: Vec<u8>,
    pub read_only: bool,
    pub fstype: String,
    pub source: Option<String>,
    /// Only filled for `Class::Real` mounts.
    pub size: Option<u64>,
    pub used: Option<u64>,
    /// Block-device stack; only probed for `Class::Real` mounts with a real device number.
    pub device: Option<topology::Device>,
}

impl Mount {
    pub fn target_path(&self) -> &Path {
        Path::new(OsStr::from_bytes(&self.target))
    }
    /// Lossy label for display only; never use it as an authority.
    pub fn target_label(&self) -> String {
        String::from_utf8_lossy(&self.target).into_owned()
    }
    pub fn class(&self) -> Class {
        classify(&self.fstype)
    }
    pub fn support(&self) -> Support {
        support(&self.fstype)
    }
}

/// mountinfo escapes space, tab, newline and backslash as \ooo octal.
fn unescape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        if field[i] == b'\\'
            && let Some(d) = field.get(i + 1..i + 4)
            && d.iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v =
                u32::from(d[0] - b'0') * 64 + u32::from(d[1] - b'0') * 8 + u32::from(d[2] - b'0');
            if let Ok(b) = u8::try_from(v) {
                out.push(b);
                i += 4;
                continue;
            }
        }
        out.push(field[i]);
        i += 1;
    }
    out
}

fn num(field: &[u8]) -> Option<u32> {
    std::str::from_utf8(field).ok()?.parse().ok()
}

fn parse_line(line: &[u8]) -> Option<Mount> {
    let f: Vec<&[u8]> = line.split(|&b| b == b' ').collect();
    let dash = f.iter().position(|x| *x == b"-")?;
    if dash < 6 || f.len() < dash + 3 {
        return None;
    }
    let (major, minor) = {
        let (a, b) = std::str::from_utf8(f[2]).ok()?.split_once(':')?;
        (a.parse().ok()?, b.parse().ok()?)
    };
    let source = String::from_utf8_lossy(&unescape(f[dash + 2])).into_owned();
    Some(Mount {
        id: num(f[0])?,
        parent_id: num(f[1])?,
        major,
        minor,
        root: unescape(f[3]),
        target: unescape(f[4]),
        read_only: f[5].split(|&b| b == b',').any(|o| o == b"ro"),
        fstype: String::from_utf8_lossy(f[dash + 1]).into_owned(),
        source: (source != "none").then_some(source),
        size: None,
        used: None,
        device: None,
    })
}

pub fn parse_mountinfo(data: &[u8]) -> Vec<Mount> {
    data.split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(parse_line)
        .collect()
}

/// (size, used) in bytes from statvfs, with checked arithmetic.
fn capacity(path: &Path) -> Option<(u64, u64)> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: zeroed statvfs is a valid out-parameter; c is NUL-terminated.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let unit = st.f_frsize;
    let size = st.f_blocks.checked_mul(unit)?;
    let used = st.f_blocks.checked_sub(st.f_bfree)?.checked_mul(unit)?;
    Some((size, used))
}

/// Mounted filesystems except pseudo ones. Unreadable mountinfo is an error, not an empty list.
pub fn discover() -> io::Result<Vec<Mount>> {
    let mut mounts: Vec<Mount> = parse_mountinfo(&fs::read("/proc/self/mountinfo")?)
        .into_iter()
        .filter(|m| m.class() != Class::Pseudo)
        .collect();
    for m in &mut mounts {
        if m.class() == Class::Real
            && let Some((size, used)) = capacity(m.target_path())
        {
            (m.size, m.used) = (Some(size), Some(used));
        }
        if m.class() == Class::Real && m.major != 0 {
            m.device = topology::probe(Path::new("/sys"), m.major, m.minor);
        }
    }
    Ok(mounts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] =
        b"22 1 259:1 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p1 rw,errors=remount-ro\n\
30 22 0:25 / /sys rw,nosuid shared:2 - sysfs sysfs rw\n\
41 22 8:17 /sub /mnt/my\\040disk ro,noatime - btrfs /dev/sdb1 ro\n\
42 22 8:33 / /mnt/\xff\\134x rw - weirdfs none rw\n\
garbage line\n";

    #[test]
    fn parses_fields_and_skips_garbage() {
        let m = parse_mountinfo(SAMPLE);
        assert_eq!(m.len(), 4);
        assert_eq!((m[0].major, m[0].minor), (259, 1));
        assert_eq!(m[0].source.as_deref(), Some("/dev/nvme0n1p1"));
        assert!(!m[0].read_only);
    }

    #[test]
    fn keeps_raw_path_bytes_and_root() {
        let m = parse_mountinfo(SAMPLE);
        assert_eq!(m[2].target, b"/mnt/my disk");
        assert_eq!(m[2].root, b"/sub");
        assert!(m[2].read_only);
        assert_eq!(m[3].target, b"/mnt/\xff\\x"); // non-UTF-8 byte and escaped backslash survive
        assert_eq!(m[3].source, None);
    }

    #[test]
    fn classification_separates_unknown_from_unsupported() {
        let m = parse_mountinfo(SAMPLE);
        assert_eq!(m[1].class(), Class::Pseudo);
        assert_eq!(m[0].support(), Support::Ext4);
        assert_eq!(support("vfat"), Support::Unsupported);
        assert_eq!(m[3].support(), Support::Unknown);
        assert_eq!(classify("nfs4"), Class::Network);
        assert_eq!(classify("fuse.sshfs"), Class::Network);
        assert_eq!(classify("weirdfs"), Class::Real);
    }

    #[test]
    fn discover_excludes_pseudo_and_finds_root() {
        let m = discover().unwrap();
        assert!(m.iter().all(|x| x.class() != Class::Pseudo));
        assert!(m.iter().any(|x| x.target == b"/"));
    }
}
