> [!WARNING]
> The database schema is not yet stable and may change between releases, requiring data to be re-collected from scratch. Do not use this for long-term collection yet. Follow emacs-devel list for announcements when it stabilises.

# emacs-reporter

Collects Emacs process metrics over time and stores them locally in a SQLite database. If you're willing to share your data, there's an uploader too.

I'm looking for long-running patterns — days, weeks, maybe even months. Short term sampling didn't uncover any anomalies helpful in improving Emacs performance on macOS.

Disclaimer: The low-level platform code and architecture decisions are mine. LLM assisted with boilerplate and Rust/macOS API wiring.

---

## Running the reporter

The reporter needs access to Emacs process internals. Two options:

```bash
# Option A — run as root
sudo ./emacs-reporter

# Option B — sign your Emacs binary (one-time, must quit Emacs first)
./sign-emacs.sh /Applications/Emacs.app
./emacs-reporter
```

Reporter takes first sample as soon as it starts and reports found errors.

The signing script works with both `.app` bundles and raw binaries (`which emacs`). Re-run it after Emacs updates.

The reporter samples every 10 minutes and writes to `emacs_reporter.db` in the current directory. It's a plain SQLite3 file — open it with SQLite3 cli, `litecli`, TablePlus, or any SQLite tool if you want to inspect what's collected.

---

## Uploading

```bash
# from the directory containing emacs_reporter.db
./emacs-report-uploader

# or with explicit path
./emacs-report-uploader /path/to/emacs_reporter.db
```

The uploader compresses the database with bzip2, assigns it a randomly generated user hash, and sends it to a Cloudflare bucket. No progress output — it either prints `done` or an error. The 50 MB post-compression limit covers months of normal usage. If your database exceeds that, contact me directly.

Note: rate limiter per ip is enabled as well as some sanity checks on the upload. File an issue if something doesn't work on reporter side.

---

## Data collected

Every sample (10 min interval) records the following:

| Table | Field | Description |
|---|---|---|
| `process` | `pid`, `start_time` | Process identity — primary key across restarts |
| `process` | `binary_path` | Full path to the Emacs binary |
| `process` | `emacs_version` | Output of `(emacs-version)` |
| `process` | `configure_options` | `system-configuration-options` at build time |
| `process` | `build_features` | `system-configuration-features` at build time |
| `cpu` | `user_ms`, `system_ms` | Cumulative CPU time since process start |
| `cpu` | `cpu_percent` | CPU utilisation over the sample interval |
| `cpu` | `messages_sent`, `messages_received` | Cumulative Mach IPC message counts since process start |
| `cpu` | `syscalls_mach`, `syscalls_unix` | Cumulative Mach and BSD syscall counts since process start |
| `cpu` | `context_switches`, `delta_csw` | Cumulative context switches and delta vs previous sample |
| `memory` | `virt_size` | Virtual memory size |
| `memory` | `resident_size` | Resident set size (RSS) |
| `memory` | `phys_footprint` | Physical footprint (Activity Monitor value) |
| `memory` | `phys_footprint_peak` | Lifetime peak physical footprint |
| `memory` | `private_size`, `shared_size` | Private vs shared memory |
| `memory` | `swapped_size` | Compressed/swapped memory |
| `memory` | `purgeable_volatile` | Purgeable volatile resident memory |
| `memory` | `purgeable_nonvolatile` | Purgeable non-volatile memory (pmap) |
| `vm_region` | `region_type` | VM region tag (`__TEXT`, `MALLOC_SMALL`, `STACK`, etc.) |
| `vm_region` | `dirty_size`, `swapped_size` | Dirty and swapped pages per region type |
| `vm_region` | `resident_size`, `virtual_size` | Resident and virtual size per region type |
| `vm_region` | `protection`, `share_mode` | Memory protection flags and share mode |
| `threads` | `thread_count`, `running_count` | Total and active thread count |
| `threads` | `faults_total`, `faults_cow` | Page faults since process start |
| `threads` | `pageins` | Page-ins since process start |
| `io` | `bytes_read`, `bytes_written` | Disk I/O since process start |
| `io` | `logical_writes` | Logical write bytes since process start |
| `ports` | `mach_port_count` | Mach port count |
| `ports` | `fd_count` | Open file descriptor count |
| `energy` | `cpu_energy_nj` | Cumulative CPU energy billed to the process, in nanojoules |
| `energy` | `gpu_time_ms` | Proxy for GPU/compositor activity (user-interactive QoS CPU time, ms) |
| `energy` | `delta_cpu_energy_nj`, `delta_gpu_time_ms` | Deltas vs previous sample |
| `display_snapshot` | `width_px`, `height_px` | Display resolution |
| `display_snapshot` | `refresh_rate` | Refresh rate in Hz |
| `display_snapshot` | `width_mm`, `height_mm` | Physical display size |
| `display_snapshot` | `is_main` | Whether this is the primary display |
| `meta` | `user_hash` | Random UUID generated once — no PII, used to deduplicate uploads |
| `meta` | `os_version` | macOS version string |

The following is recorded once, not per sample:

| Table | Field | Description |
|---|---|---|
| `system_info` | `total_ram` | Total physical RAM in bytes |
| `system_info` | `cpu_arch` | CPU architecture (e.g. `arm64`) |
| `system_info` | `cpu_cores` | Logical CPU count |
| `system_info` | `hw_model` | Hardware model identifier (e.g. `Mac14,3`) |
| `system_info` | `cpu_brand` | CPU brand string (e.g. `Apple M2`) |
| `installed_package` | `name`, `version` | Name and version of each installed ELPA package |
| `process_package` | `process_id`, `package_id` | Links each Emacs process to its installed packages |

System info is recorded on first launch. Package list is recorded when an Emacs process is first seen — package.el installs to `~/.emacs.d/elpa` (or the XDG equivalent), so packages installed via other mechanisms (straight.el, Borg, manual) are not captured.

Nothing outside this table is collected. The database stays on your machine until you run the uploader.

## Privacy

Once data starts flowing I'll periodically merge all submitted databases into a single dataset for analysis. The only potentially identifying field is `binary_path` — the path to your Emacs binary. If you built or installed Emacs somewhere under your home directory, this will contain your username (e.g. `/Users/john/...`). I'll replace that prefix with `$HOME` before publishing any merged dataset, but you should be aware of this before submitting. If your Emacs is installed via Homebrew or `/Applications` — which covers most cases — no username is present.
