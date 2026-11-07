# ExtentPilot

An early Linux desktop utility for **read-only extent analysis**. This development build scans files and directories, reports real FIEMAP measurements, and shows a logical file schematic and searchable measurement table. Optimization is not yet implemented.

## Run

Requires Linux 5.6 or newer (openat2), mounted procfs, Rust 1.95+, util-linux `findmnt`, and a Wayland or X11 session for the GUI.

```console
cargo run
cargo run -- drives
cargo run -- analyze /path/to/directory
cargo run -- analyze /path/to/file --json > report.json
```

The GUI refuses to run as root. Scans use the current user's permissions. If the Cargo cache is read-only, prefix Cargo commands with `CARGO_HOME=/tmp/extentpilot-cargo`.

## Analysis

- Retrieves 128 extent records per batch without FIEMAP_SYNC or file modification.
- Reports logical/allocated size, extent count, average and largest extent, sharing/uncertainty indicators, and measurement completeness.
- Deduplicates hard links. Preserves raw filename bytes in JSON alongside display strings.
- Rejects symlinks, special files and nested mounts, including bind mounts. Traverses held directory descriptors.
- Supports cooperative cancellation between entries and ioctl batches. Partial results remain visible.
- Bounds work to 100,000 entries, 128 directory levels, 5,000 detail rows and 100 diagnostics. Totals cover all measured files; results explicitly report limits and omitted rows.

The schematic shows a bounded prefix of retained files, one cell per file. Gray means uncertain or incomplete, purple a shared-extent indicator, amber multiple reported extents, teal zero or one reported extent. It is not a physical disk map or performance score. Missing sharing indicators do not establish that extents are unshared. Completeness describes retrieval and observed metadata stability, not an atomic snapshot.

JSON reports contain filenames: store them privately. File contents are never exported.

## Validate

```console
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

See [testing](docs/testing.md), [architecture](docs/architecture.md), and [limitations](docs/limitations.md). Privileged backends, history, scheduling, packaging and stable-release qualification remain outstanding.
