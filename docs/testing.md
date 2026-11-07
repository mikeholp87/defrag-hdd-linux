# Analysis testing

Run `cargo test` on a FIEMAP-capable project filesystem such as ext4. Temporary fixtures are created under the project directory and automatically removed. Some memory filesystems, including this environment's /tmp, reject FIEMAP with EOPNOTSUPP. That means unavailable measurement, not zero fragmentation. Tests require actual FIEMAP support rather than silently skipping checks.

Tests cover flags, sparse allocation, checked arithmetic, pagination across 128-extent batches, malformed kernel responses, hard-link deduplication, non-UTF-8 export, FIFO and symlink rejection, cancellation before traversal, nested-mount rejection and bounded retention. The mount test requires /proc mounted separately from /, as in a normal Linux session.

The file fixture checks byte-for-byte content preservation after scanning. It does not qualify a modifying backend. No filesystem formatting, defragmentation, TRIM or privileged helper test has run.

Manual workflow: launch `cargo run`, enter a small directory, start a scan, inspect the schematic and table, filter by filename, and expand exclusions. Cancel a larger scan and verify the cancelled state and retained partial results. Close during a scan to check cancellation on shutdown. Export with `cargo run -- analyze PATH --json`.

KDE/GNOME, scaling, large-directory performance and cancellation during a kernel call require further desktop validation. Unit tests alone do not establish those behaviors.
