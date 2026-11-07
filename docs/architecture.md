# Current architecture

`crates/analysis` (`extentpilot-analysis`) owns traversal and FIEMAP measurements, without GUI dependencies. Kernel ioctl numbers and flags come from linux-raw-sys. repr(C) structures reproduce linux/fiemap.h including reserved fields; compile-time assertions check extent size and flexible-array offset. Unsafe calls stay at Linux API boundaries with checked counts and arithmetic.

openat2 rejects symlinks. Descendants additionally require BENEATH and NO_XDEV. O_PATH pins objects before type validation; procfs reopens only files and directories for reading. Directory enumeration uses held descriptors. Renames may make display paths stale; those labels are never privileged authority.

`crates/app` (binary `extentpilot`) shares analysis between CLI and GUI. One worker sends its final result through a bounded channel. Atomic counters provide progress without per-file message queues; an atomic flag requests cancellation. The GUI polls during scans and virtualizes table rows. Closing requests cancellation. `crates/devices` (`extentpilot-devices`) parses mountinfo into raw-byte mounts with a filesystem class and backend-support classification, plus a sysfs block-device stack (`topology` module, rotational/discard as reported, unknown never guessed); discovery still runs synchronously at startup.

The workspace has three crates: analysis, devices, app. The remaining requested crates (core, backends, jobs, security, visuals, history, helper) are not created yet; each is added when it has real code. Capability/topology models remain outstanding.
