# Development limitations

This read-only analysis build is not a stable maintenance release.

FIEMAP observations are not atomic. Size, mtime and ctime checks detect some changes but cannot prove files remained unchanged. Flags other than LAST and SHARED conservatively indicate uncertainty. Missing SHARED flags do not prove absence of snapshots, reflinks or deduplication. Physical addresses never drive raw I/O or represent complete disk allocation.

Unsupported ioctls and permissions produce unavailable measurements. Scans exclude nested mounts and symlinks. Btrfs subvolume boundaries may not be mount boundaries; a dedicated policy is unimplemented. No filesystem-wide completeness is promised.

Resource budgets are fixed defaults. Detail rows retain the first measurements, not necessarily the most fragmented files. The schematic displays a bounded prefix. Cancellation cannot interrupt a blocked kernel call. CLI interruption currently terminates normally rather than exporting partial results.

Discovery parses /proc/self/mountinfo (raw mount-path bytes, bind/subvolume root kept) and runs on the GUI thread at startup. Pseudo filesystems are hidden; network and FUSE mounts are listed but never statted. Filesystem classes and backend support (Ext4/Xfs/Btrfs, Unsupported, Unknown) come from a fixed fstype table, not kernel capability probing. Capacity comes from statvfs. Block-device topology comes from sysfs (`/sys/dev/block`): partitions use the parent disk's `queue/rotational` and `discard_max_bytes`; dm/md members are followed through `slaves/` to a depth of 8, and an unreadable member makes the whole result unknown. Values are kernel-reported and can be wrong for VMs, USB bridges and RAID controllers. Loop devices never claim media. Btrfs/NFS-style anonymous devices (major 0) get no topology, and multi-device Btrfs is not resolved via /sys/fs/btrfs. Discard is the top layer's own limit; it does not prove TRIM works end to end. Mount-ID reuse tracking, filesystem-specific recommendations, configurable budgets, settings, history, comparisons, scheduling, notifications, privileged operations and packaging remain incomplete.

No modifying ext4/XFS/Btrfs backend or TRIM operation is enabled. No integrity after modification, performance benefit, space recovery or release readiness is claimed.
