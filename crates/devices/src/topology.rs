//! Block-device stack from sysfs. Read-only; every value is "as reported by the kernel".
//! Virtual or emulated devices may misreport rotational/discard, so this is evidence, not proof.
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

const DEPTH_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Disk,
    Partition,
    DeviceMapper,
    Md,
    /// Backing file is not traced, so its media is never claimed.
    Loop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Rotational,
    NonRotational,
    Mixed,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub kind: Kind,
    pub major: u32,
    pub minor: u32,
    /// Kernel `queue/rotational`. None = unreadable.
    pub rotational: Option<bool>,
    /// This layer's own `queue/discard_max_bytes > 0`; what fstrim on this layer would see.
    pub discard: Option<bool>,
    /// Underlying devices (dm/md members). Empty for a plain disk or partition.
    pub children: Vec<Device>,
    /// True when the stack was cut at the depth limit or a member could not be read.
    pub incomplete: bool,
}

impl Device {
    /// Combined media over the leaves. Any unreadable or untraceable leaf makes it Unknown.
    pub fn media(&self) -> Media {
        if self.incomplete || self.kind == Kind::Loop {
            return Media::Unknown;
        }
        if self.children.is_empty() {
            return match self.rotational {
                Some(true) => Media::Rotational,
                Some(false) => Media::NonRotational,
                None => Media::Unknown,
            };
        }
        let mut media = self.children.iter().map(Device::media);
        let first = media.next().unwrap_or(Media::Unknown);
        media.fold(first, |a, b| match (a, b) {
            (Media::Unknown, _) | (_, Media::Unknown) => Media::Unknown,
            (a, b) if a == b => a,
            _ => Media::Mixed,
        })
    }
}

impl Media {
    pub fn label(self) -> &'static str {
        match self {
            Media::Rotational => "rotational",
            Media::NonRotational => "non-rotational",
            Media::Mixed => "mixed",
            Media::Unknown => "unknown",
        }
    }
}

impl Device {
    /// One line for UI/CLI: stack, media and top-layer discard, all "as reported".
    pub fn summary(&self) -> String {
        let discard = match self.discard {
            Some(true) => "discard reported",
            Some(false) => "no discard reported",
            None => "discard unknown",
        };
        format!("{self} · {} · {discard}", self.media().label())
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:?})", self.name, self.kind)?;
        if !self.children.is_empty() {
            f.write_str(" on [")?;
            for (i, c) in self.children.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{c}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

fn read_trim(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn flag(path: PathBuf) -> Option<bool> {
    match read_trim(&path)?.as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn nonzero(path: PathBuf) -> Option<bool> {
    Some(read_trim(&path)?.parse::<u64>().ok()? > 0)
}

fn parse_dev(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.trim().split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Probe the device `major:minor` under a sysfs root (`/sys` in production).
/// None when it has no block-device node (e.g. btrfs/NFS anonymous devices, major 0).
pub fn probe(sys: &Path, major: u32, minor: u32) -> Option<Device> {
    probe_at(sys, major, minor, 0)
}

fn probe_at(sys: &Path, major: u32, minor: u32, depth: usize) -> Option<Device> {
    let dir = fs::canonicalize(sys.join(format!("dev/block/{major}:{minor}"))).ok()?;
    let name = dir.file_name()?.to_string_lossy().into_owned();
    let is_partition = dir.join("partition").exists();
    // Queue attributes live on the whole disk, which is the partition's parent directory.
    let queue_dir = if is_partition { dir.parent()? } else { &dir };
    let kind = if is_partition {
        Kind::Partition
    } else if dir.join("dm").is_dir() {
        Kind::DeviceMapper
    } else if dir.join("md").is_dir() {
        Kind::Md
    } else if dir.join("loop").is_dir() {
        Kind::Loop
    } else {
        Kind::Disk
    };
    let mut device = Device {
        name,
        kind,
        major,
        minor,
        rotational: flag(queue_dir.join("queue/rotational")),
        discard: nonzero(queue_dir.join("queue/discard_max_bytes")),
        children: Vec::new(),
        incomplete: false,
    };
    if let Ok(members) = fs::read_dir(dir.join("slaves")) {
        if depth >= DEPTH_LIMIT {
            device.incomplete = true;
            return Some(device);
        }
        let mut names: Vec<_> = members.filter_map(Result::ok).map(|e| e.path()).collect();
        names.sort();
        for member in names {
            match read_trim(&member.join("dev"))
                .as_deref()
                .and_then(parse_dev)
                .and_then(|(a, b)| probe_at(sys, a, b, depth + 1))
            {
                Some(child) => device.children.push(child),
                None => device.incomplete = true,
            }
        }
    }
    Some(device)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Fake sysfs: add_dev creates devices/<name> and dev/block/<maj:min> -> it.
    struct Sys(tempfile::TempDir);
    impl Sys {
        fn new() -> Self {
            let s = Sys(tempfile::tempdir().unwrap());
            fs::create_dir_all(s.0.path().join("dev/block")).unwrap();
            s
        }
        fn add(&self, rel: &str, id: &str, files: &[(&str, &str)]) -> PathBuf {
            let dir = self.0.path().join("devices").join(rel);
            fs::create_dir_all(&dir).unwrap();
            for (f, v) in files {
                let p = dir.join(f);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, v).unwrap();
            }
            symlink(&dir, self.0.path().join("dev/block").join(id)).unwrap();
            dir
        }
        fn member(&self, parent: &Path, name: &str, id: &str) {
            let slave = parent.join("slaves").join(name);
            fs::create_dir_all(&slave).unwrap();
            fs::write(slave.join("dev"), id).unwrap();
        }
    }
    const HDD: &[(&str, &str)] = &[
        ("queue/rotational", "1\n"),
        ("queue/discard_max_bytes", "0\n"),
    ];
    const SSD: &[(&str, &str)] = &[
        ("queue/rotational", "0\n"),
        ("queue/discard_max_bytes", "4096\n"),
    ];

    #[test]
    fn partition_uses_parent_disk_queue() {
        let s = Sys::new();
        s.add("sda", "8:0", HDD);
        s.add("sda/sda1", "8:1", &[("partition", "1\n")]);
        let d = probe(s.0.path(), 8, 1).unwrap();
        assert_eq!((d.kind, d.name.as_str()), (Kind::Partition, "sda1"));
        assert_eq!(d.media(), Media::Rotational);
        assert_eq!(d.discard, Some(false));
    }

    #[test]
    fn stacked_dm_aggregates_members() {
        let s = Sys::new();
        s.add("nvme0n1", "259:0", SSD);
        s.add("sda", "8:0", HDD);
        let dm = s.add(
            "dm-0",
            "253:0",
            &[("dm/name", "x\n"), ("queue/discard_max_bytes", "0\n")],
        );
        s.member(&dm, "nvme0n1", "259:0");
        assert_eq!(
            probe(s.0.path(), 253, 0).unwrap().media(),
            Media::NonRotational
        );
        s.member(&dm, "sda", "8:0");
        let d = probe(s.0.path(), 253, 0).unwrap();
        assert_eq!(
            (d.kind, d.media(), d.children.len()),
            (Kind::DeviceMapper, Media::Mixed, 2)
        );
        assert_eq!(d.discard, Some(false)); // the dm layer's own limit, not its members'
    }

    #[test]
    fn unreadable_member_or_missing_attribute_is_unknown_not_guessed() {
        let s = Sys::new();
        s.add("sdb", "8:16", &[]); // no queue/rotational
        assert_eq!(probe(s.0.path(), 8, 16).unwrap().media(), Media::Unknown);
        let md = s.add("md0", "9:0", &[("md/level", "raid1\n")]);
        s.member(&md, "ghost", "8:99"); // member without a block node
        let d = probe(s.0.path(), 9, 0).unwrap();
        assert!(d.incomplete);
        assert_eq!(d.media(), Media::Unknown);
    }

    #[test]
    fn loop_media_is_never_claimed_and_anonymous_devices_have_none() {
        let s = Sys::new();
        s.add(
            "loop0",
            "7:0",
            &[("loop/backing_file", "/x\n"), ("queue/rotational", "0\n")],
        );
        let d = probe(s.0.path(), 7, 0).unwrap();
        assert_eq!((d.kind, d.media()), (Kind::Loop, Media::Unknown));
        assert!(probe(s.0.path(), 0, 42).is_none());
    }

    #[test]
    fn membership_cycle_stops_at_depth_limit() {
        let s = Sys::new();
        let a = s.add("dm-0", "253:0", &[("dm/name", "a\n")]);
        s.member(&a, "dm-0", "253:0"); // member points back at itself
        let d = probe(s.0.path(), 253, 0).unwrap();
        assert_eq!(d.media(), Media::Unknown);
    }
}
