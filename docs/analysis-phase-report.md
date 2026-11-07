# Read-only analysis phase

Implemented chunked FIEMAP measurement, checked response parsing, directory traversal, hard-link deduplication, shared/uncertain flags, partial results, cooperative GUI cancellation, bounded work/detail retention, a searchable virtualized table, and a logical schematic with a legend and file hover labels. CLI `analyze PATH --json` uses the same scanner and preserves raw filename bytes.

Files created: `src/analysis.rs`, `.gitignore`, `docs/architecture.md`, `docs/testing.md`, `docs/limitations.md`, and this report. Updated: `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `README.md`.

Added direct dependency linux-raw-sys 0.12.1 for authoritative ioctl constants, and test dependency tempfile 3.27.0. Existing locked direct dependencies are eframe 0.37.0, serde 1.0.229, serde_json 1.0.154 and libc 0.2.191. The scanner uses Linux openat2 and procfs; it fails closed if these are unavailable.

Verification on 2026-11-07:

- `cargo test`: 15 passed, zero failed. Fixtures on the project filesystem exercised real FIEMAP. Initial fixtures on /tmp exposed unsupported FIEMAP and failed; they were moved to the project filesystem, not silently skipped.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo build --release`: passed. Release CLI checks passed for directory JSON scanning, special-file rejection (exit 1), and invalid commands (exit 2).
- `cargo audit --file Cargo.lock`: scanned 325 locked packages against 1,422 advisories; exited successfully without reported vulnerabilities.
- Gitleaks scans of source, Cargo.toml and Cargo.lock: no leaks found.
- A compiled C assertion check against installed Linux UAPI headers confirmed FIEMAP extent size, header offset and ioctl number. The application and its tests remain Rust.
- Live `extentpilot analyze src --json`: two measured and complete files, two reported extents, no cancellation or resource-limit flag.
- X11 launch produced a viewable application window, captured and visually inspected. Graphics driver warnings occurred. Automated input did not establish successful GUI scanning, so end-to-end desktop interaction remains unverified.
- Jev Review could not run: its tool reports that JEV_API_KEY is unset.

No modifying backend, privileged helper, TRIM, filesystem-image integration, Debian packaging, performance benchmark or full desktop compatibility test was run. No real storage was formatted or optimized. See limitations for remaining architecture and product work.
