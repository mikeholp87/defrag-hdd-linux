# ExtentPilot

**Read-only Linux extent analysis** — a development build that scans files and directories,
reports real FIEMAP measurements, and shows a logical file schematic plus a searchable
measurement table. **No optimization is implemented yet.** Defragmentation, TRIM, history,
scheduling and privileged backends remain outstanding.

Repository: `defrag-hdd-linux` — https://github.com/mikeholp87/defrag-hdd-linux.git

## What it is

ExtentPilot inspects storage without modifying anything. It:

- Measures real extents via the Linux `FIEMAP` ioctl (read-only, no file modification)
- Walks directory trees using held descriptors and `openat2`, rejecting symlinks,
  special files and nested mounts (including bind mounts)
- Deduplicates hard links by device:inode; preserves raw filename bytes in JSON output
- Reports logical/allocated size, extent count, average and largest extent, shared and
  uncertain flags, and measurement completeness
- Bounds work to 100,000 entries, 128 directory levels, 5,000 detail rows and 100
  diagnostics; totals cover all measured files and limits/omitted rows are reported
- Supports cooperative cancellation between entries and ioctl batches; partial results
  remain visible
- Renders a logical schematic (one cell per retained file) and a searchable virtualized
  table in the GUI, plus a JSON report from the CLI

The schematic is **not** a physical disk map or performance score. Colors: gray =
uncertain/incomplete, purple = shared-extent indicator, amber = multiple reported extents,
teal = zero or one reported extent. Missing sharing indicators do not establish that
extents are unshared. Completeness describes retrieval and observed metadata stability,
not an atomic snapshot.

JSON reports contain filenames: store them privately. File contents are never exported.

## Workspace

Three crates:

| Crate | Purpose |
|---|---|
| `extentpilot-analysis` (`crates/analysis`) | Traversal and FIEMAP measurement; no GUI dependencies |
| `extentpilot-devices` (`crates/devices`) | Mountinfo parsing, filesystem class/backend-support classification, sysfs block-device topology |
| `extentpilot` (`crates/app`) | Binary `extentpilot`; shares analysis between CLI and egui GUI |

Requested but not-yet-created crates: `core`, `backends`, `jobs`, `security`, `visuals`,
`history`, `helper`. Each is added when it has real code.

## Requirements

- Linux 5.6 or newer (openat2 `RESOLVE_BENEATH` / `RESOLVE_NO_XDEV`)
- Mounted procfs (`/proc/self/mountinfo`, `/proc/self/fd/`)
- Rust 1.95+ (edition 2024)
- util-linux `findmnt` (for `drives` command reference)
- A Wayland or X11 session for the GUI

## Build and run

```console
cargo build --release
cargo run                          # GUI
cargo run -- drives                # CLI: list mounted filesystems
cargo run -- analyze /path/to/directory
cargo run -- analyze /path/to/file --json > report.json
cargo run -- --help
cargo run -- --version
```

The GUI refuses to run as root. Scans use the current user's permissions. If the Cargo
cache is read-only, prefix Cargo commands with `CARGO_HOME=/tmp/extentpilot-cargo`.

### CLI commands

- `extentpilot [drives | analyze PATH [--json] | --version]`
- `drives` — print one line per non-pseudo mount: target, fstype, source, media label
- `analyze PATH` — scan a file or directory; print a text summary
- `analyze PATH --json` — pretty JSON report with per-file rows and raw filename bytes
- `--help` / `--version`

## Analysis

- Retrieves 128 extent records per batch without `FIEMAP_SYNC` or file modification
- Kernel ioctl numbers and flags come from `linux-raw-sys`; `repr(C)` structures reproduce
  `linux/fiemap.h` including reserved fields, with compile-time layout assertions
- `openat2` rejects symlinks; descendants additionally require `BENEATH` and `NO_XDEV`.
  `O_PATH` pins objects before type validation; procfs reopens only files and directories
  for reading
- Unsafe calls stay at Linux API boundaries with checked counts and arithmetic
- One worker sends its final result through a bounded channel; atomic counters provide
  progress without per-file message queues; an atomic flag requests cancellation

## Device discovery

`discover()` parses `/proc/self/mountinfo` (raw mount-path bytes; bind/subvolume roots
kept), excludes pseudo filesystems, and for real mounts reports:

- capacity and used bytes from `statvfs`
- block-device topology from sysfs (`/sys/dev/block`): partitions use the parent disk's
  `queue/rotational` and `discard_max_bytes`; dm/md members followed through `slaves/`
  to a depth of 8; unreadable members make the whole result unknown
- filesystem class (Pseudo / Network / Real) and backend support (Ext4 / Xfs / Btrfs /
  Unsupported / Unknown) from a fixed fstype table, not kernel capability probing

Values are kernel-reported and can be wrong for VMs, USB bridges and RAID controllers.
Loop devices never claim media. Btrfs/NFS-style anonymous devices (major 0) get no
topology, and multi-device Btrfs is not resolved via `/sys/fs/btrfs`. Discard is the top
layer's own limit; it does not prove TRIM works end to end.

## Limitations

This read-only analysis build is **not** a stable maintenance release.

- FIEMAP observations are not atomic. Size, mtime and ctime checks detect some changes
  but cannot prove files remained unchanged. Flags other than `LAST` and `SHARED`
  conservatively indicate uncertainty. Missing `SHARED` flags do not prove absence of
  snapshots, reflinks or deduplication. Physical addresses never drive raw I/O or
  represent complete disk allocation.
- Unsupported ioctls and permissions produce unavailable measurements. Scans exclude
  nested mounts and symlinks. Btrfs subvolume boundaries may not be mount boundaries;
  a dedicated policy is unimplemented. No filesystem-wide completeness is promised.
- Resource budgets are fixed defaults. Detail rows retain the first measurements, not
  necessarily the most fragmented files. The schematic displays a bounded prefix.
  Cancellation cannot interrupt a blocked kernel call. CLI interruption currently
  terminates normally rather than exporting partial results.
- Discovery runs on the GUI thread at startup. Pseudo filesystems are hidden; network
  and FUSE mounts are listed but never statted. Filesystem classes and backend support
  come from a fixed fstype table, not kernel capability probing. Capacity comes from
  `statvfs`. Block-device topology comes from sysfs; values can be wrong for VMs, USB
  bridges and RAID controllers. Loop devices never claim media. Btrfs/NFS-style
  anonymous devices (major 0) get no topology, and multi-device Btrfs is not resolved via
  `/sys/fs/btrfs`. Discard is the top layer's own limit; it does not prove TRIM works
  end to end.
- Mount-ID reuse tracking, filesystem-specific recommendations, configurable budgets,
  settings, history, comparisons, scheduling, notifications, privileged operations and
  packaging remain incomplete.
- **No modifying ext4/XFS/Btrfs backend or TRIM operation is enabled. No integrity after
  modification, performance benefit, space recovery or release readiness is claimed.**

## Testing

Run `cargo test` on a FIEMAP-capable project filesystem such as ext4. Temporary fixtures
are created under the project directory and automatically removed. Some memory
filesystems, including this environment's `/tmp`, reject FIEMAP with `EOPNOTSUPP` — that
means unavailable measurement, not zero fragmentation. Tests require actual FIEMAP
support rather than silently skipping checks.

Tests cover flags, sparse allocation, checked arithmetic, pagination across 128-extent
batches, malformed kernel responses, hard-link deduplication, non-UTF-8 export, FIFO and
symlink rejection, cancellation before traversal, nested-mount rejection and bounded
retention. The mount test requires `/proc` mounted separately from `/`, as in a normal
Linux session.

The file fixture checks byte-for-byte content preservation after scanning. It does not
qualify a modifying backend. No filesystem formatting, defragmentation, TRIM or
privileged-helper test has run.

Manual workflow: launch `cargo run`, enter a small directory, start a scan, inspect the
schematic and table, filter by filename, and expand exclusions. Cancel a larger scan and
verify the cancelled state and retained partial results. Close during a scan to check
cancellation on shutdown. Export with `cargo run -- analyze PATH --json`.

KDE/GNOME, scaling, large-directory performance and cancellation during a kernel call
require further desktop validation. Unit tests alone do not establish those behaviors.

## Validate

```console
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

## Verification (2026-11-07)

- `cargo test`: 15 passed, zero failed. Fixtures on the project filesystem exercised real
  FIEMAP. Initial fixtures on `/tmp` exposed unsupported FIEMAP and failed; they were
  moved to the project filesystem, not silently skipped.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo build --release`: passed. Release CLI checks passed for directory JSON scanning,
  special-file rejection (exit 1), and invalid commands (exit 2).
- `cargo audit --file Cargo.lock`: scanned 325 locked packages against 1,422 advisories;
  exited successfully without reported vulnerabilities.
- Gitleaks scans of source, `Cargo.toml` and `Cargo.lock`: no leaks found.
- A compiled C assertion check against installed Linux UAPI headers confirmed FIEMAP
  extent size, header offset and ioctl number. The application and its tests remain Rust.
- Live `extentpilot analyze src --json`: two measured and complete files, two reported
  extents, no cancellation or resource-limit flag.
- X11 launch produced a viewable application window, captured and visually inspected.
  Graphics driver warnings occurred. Automated input did not establish successful GUI
  scanning, so end-to-end desktop interaction remains unverified.

No modifying backend, privileged helper, TRIM, filesystem-image integration, Debian
packaging, performance benchmark or full desktop compatibility test was run. No real
storage was formatted or optimized.

## License

MIT

See [testing](docs/testing.md), [architecture](docs/architecture.md), and
[limitations](docs/limitations.md). Privileged backends, history, scheduling, packaging
and stable-release qualification remain outstanding.