//#![xcfg(target_os = "macos")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use tokio::time;
use tracing::{error, info, warn};

mod display_data;
mod extra_data;
mod platform;

// ── Configuration ─────────────────────────────────────────────────────────────

const SAMPLE_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DB_PATH: &str = "emacs_reporter.db";
const REPORTER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProcessKey {
    pid: i32,
    start_time: i64,
}

#[derive(Debug)]
struct ProcessInfo {
    db_id: i64,
    binary_path: PathBuf,
}

struct CpuBaseline {
    user_ms: i64,
    system_ms: i64,
    sampled_at: i64,
}

struct ExtraBaseline {
    faults_total: i64,
    faults_cow: i64,
    pageins: i64,
    bytes_read: i64,
    bytes_written: i64,
    logical_writes: i64,
}

struct State {
    conn: Connection,
    known: HashMap<ProcessKey, ProcessInfo>,
    cpu_baselines: HashMap<i64, CpuBaseline>,
    extra_baselines: HashMap<i64, ExtraBaseline>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let conn = open_db(DB_PATH)?;
    ensure_schema(&conn)?;
    ensure_meta(&conn)?;

    let mut state = State {
        conn,
        known: HashMap::new(),
        cpu_baselines: HashMap::new(),
        extra_baselines: HashMap::new(),
    };

    info!(
        "emacs-reporter started, interval = {}s",
        SAMPLE_INTERVAL.as_secs()
    );

    loop {
        tokio::time::sleep(SAMPLE_INTERVAL).await;
        if let Err(e) = collect_snapshot(&mut state) {
            error!("snapshot failed: {e:#}");
        }
    }
}

// ── SQLite setup ──────────────────────────────────────────────────────────────

fn open_db(path: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;

    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        anyhow::bail!("database integrity check failed: {integrity}");
    }

    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        PRAGMA synchronous = FULL;
        PRAGMA wal_checkpoint(TRUNCATE);
    ",
    )?;

    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            id               INTEGER PRIMARY KEY CHECK (id = 1),
            user_hash        TEXT NOT NULL,
            os_version       TEXT NOT NULL,
            reporter_version TEXT NOT NULL,
            created_at       INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS process (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            pid                INTEGER NOT NULL,
            start_time         INTEGER NOT NULL,
            binary_path        TEXT,
            emacs_version      TEXT,
            configure_options  TEXT,
            build_features     TEXT,
            first_seen_at      INTEGER NOT NULL,
            last_seen_at       INTEGER NOT NULL,
            UNIQUE (pid, start_time)
        );

        CREATE TABLE IF NOT EXISTS sample (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            process_id  INTEGER NOT NULL REFERENCES process (id),
            sampled_at  INTEGER NOT NULL,
            UNIQUE (process_id, sampled_at)
        );

        CREATE TABLE IF NOT EXISTS cpu (
            sample_id       INTEGER PRIMARY KEY REFERENCES sample (id),
            user_ms         INTEGER NOT NULL,
            system_ms       INTEGER NOT NULL,
            delta_user_ms   INTEGER,
            delta_system_ms INTEGER,
            interval_ms     INTEGER,
            cpu_percent     REAL
        );

        CREATE TABLE IF NOT EXISTS memory (
            sample_id           INTEGER PRIMARY KEY REFERENCES sample (id),
            virt_size           INTEGER NOT NULL,
            resident_size       INTEGER NOT NULL,
            resident_size_peak  INTEGER NOT NULL,
            phys_footprint      INTEGER NOT NULL,
            phys_footprint_peak INTEGER NOT NULL,
            private_size        INTEGER,
            shared_size         INTEGER,
            swapped_size        INTEGER
        );

        CREATE TABLE IF NOT EXISTS vm_region (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            sample_id     INTEGER NOT NULL REFERENCES sample (id),
            region_type   TEXT NOT NULL,
            region_count  INTEGER NOT NULL,
            block_count   INTEGER,
            virtual_size  INTEGER NOT NULL,
            resident_size INTEGER NOT NULL,
            dirty_size    INTEGER NOT NULL,
            swapped_size  INTEGER NOT NULL,
            shared_size   INTEGER,
            private_size  INTEGER,
            protection    TEXT,
            share_mode    TEXT,
            UNIQUE (sample_id, region_type)
        );

        CREATE TABLE IF NOT EXISTS threads (
            sample_id      INTEGER PRIMARY KEY REFERENCES sample (id),
            thread_count   INTEGER NOT NULL,
            running_count  INTEGER NOT NULL,
            faults_total   INTEGER NOT NULL,
            faults_cow     INTEGER NOT NULL,
            pageins        INTEGER NOT NULL,
            delta_faults   INTEGER,
            delta_cow      INTEGER,
            delta_pageins  INTEGER
        );

        CREATE TABLE IF NOT EXISTS io (
            sample_id             INTEGER PRIMARY KEY REFERENCES sample (id),
            bytes_read            INTEGER NOT NULL,
            bytes_written         INTEGER NOT NULL,
            logical_writes        INTEGER NOT NULL,
            delta_bytes_read      INTEGER,
            delta_bytes_written   INTEGER,
            delta_logical_writes  INTEGER
        );

        CREATE TABLE IF NOT EXISTS ports (
            sample_id         INTEGER PRIMARY KEY REFERENCES sample (id),
            mach_port_count   INTEGER NOT NULL,
            fd_count          INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS display_snapshot (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            sampled_at     INTEGER NOT NULL,
            display_index  INTEGER NOT NULL,
            width_px       INTEGER NOT NULL,
            height_px      INTEGER NOT NULL,
            refresh_rate   REAL,
            width_mm       INTEGER,
            height_mm      INTEGER,
            is_main        INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sample_display (
            sample_id           INTEGER NOT NULL REFERENCES sample (id),
            display_snapshot_id INTEGER NOT NULL REFERENCES display_snapshot (id),
            PRIMARY KEY (sample_id, display_snapshot_id)
        );

        CREATE INDEX IF NOT EXISTS idx_sample_process    ON sample (process_id, sampled_at);
        CREATE INDEX IF NOT EXISTS idx_vm_region_sample  ON vm_region (sample_id);
        CREATE INDEX IF NOT EXISTS idx_vm_region_type    ON vm_region (region_type, sample_id);
        CREATE INDEX IF NOT EXISTS idx_sample_display    ON sample_display (sample_id);

        CREATE VIEW IF NOT EXISTS vm_region_delta AS
        SELECT
            r.sample_id,
            s.sampled_at,
            s.process_id,
            r.region_type,
            r.dirty_size,
            r.dirty_size - LAG(r.dirty_size) OVER (
                PARTITION BY s.process_id, r.region_type
                ORDER BY s.sampled_at
            ) AS dirty_delta,
            r.resident_size,
            r.resident_size - LAG(r.resident_size) OVER (
                PARTITION BY s.process_id, r.region_type
                ORDER BY s.sampled_at
            ) AS resident_delta
        FROM vm_region r
        JOIN sample s ON s.id = r.sample_id;
    ",
    )?;

    Ok(())
}

fn ensure_meta(conn: &Connection) -> anyhow::Result<()> {
    let exists: bool = conn.query_row("SELECT COUNT(*) FROM meta WHERE id = 1", [], |r| {
        r.get::<_, i64>(0)
    })? > 0;

    if !exists {
        let user_hash = platform::generate_user_uuid();
        let os_version = platform::os_version();
        let now = unix_now();
        conn.execute(
            "INSERT INTO meta (id, user_hash, os_version, reporter_version, created_at)
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![user_hash, os_version, REPORTER_VERSION, now],
        )?;
    }

    Ok(())
}

// ── Snapshot ──────────────────────────────────────────────────────────────────

fn collect_snapshot(state: &mut State) -> anyhow::Result<()> {
    let pids = platform::find_emacs_pids()?;
    if pids.is_empty() {
        info!("no Emacs processes found");
        return Ok(());
    }

    let now = unix_now();

    // Collect display state once per cycle
    let displays = match display_data::collect_displays() {
        Ok(d) => d,
        Err(e) => {
            warn!("display collection failed: {e:#}");
            vec![]
        }
    };

    // Insert display snapshot rows and collect their ids
    let display_ids = insert_display_snapshots(&state.conn, now, &displays)?;

    for pid in pids {
        match collect_process(state, pid, now, &display_ids) {
            Ok(()) => {}
            Err(e) => warn!("pid {pid}: {e:#}"),
        }
    }

    Ok(())
}

fn insert_display_snapshots(
    conn: &Connection,
    now: i64,
    displays: &[display_data::DisplayInfo],
) -> anyhow::Result<Vec<i64>> {
    let mut ids = Vec::with_capacity(displays.len());
    for display in displays {
        conn.execute(
            "INSERT INTO display_snapshot
             (sampled_at, display_index, width_px, height_px, refresh_rate,
              width_mm, height_mm, is_main)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now,
                display.display_index,
                display.width_px,
                display.height_px,
                display.refresh_rate,
                display.width_mm,
                display.height_mm,
                display.is_main as i64,
            ],
        )?;
        ids.push(conn.last_insert_rowid());
    }
    Ok(ids)
}

fn collect_process(
    state: &mut State,
    pid: i32,
    now: i64,
    display_ids: &[i64],
) -> anyhow::Result<()> {
    let start_time = platform::process_start_time(pid)?;
    let key = ProcessKey { pid, start_time };

    if !state.known.contains_key(&key) {
        let info = bootstrap_process(&state.conn, pid, start_time, now)?;
        state.known.insert(key.clone(), info);
    }

    let process_db_id = state.known[&key].db_id;

    state.conn.execute(
        "UPDATE process SET last_seen_at = ?1 WHERE id = ?2",
        params![now, process_db_id],
    )?;

    // Collect all platform data before opening transaction
    let cpu_data = platform::collect_cpu(pid)?;
    let mem_data = platform::collect_memory(pid)?;
    let regions = platform::collect_vm_regions(pid)?;
    let thread_data = extra_data::collect_threads_and_faults(pid)?;
    let io_data = extra_data::collect_io(pid)?;
    let port_data = extra_data::collect_ports_and_fds(pid)?;

    // CPU deltas
    let cpu_baseline = state.cpu_baselines.get(&process_db_id);
    let (delta_user, delta_system, interval_ms, cpu_percent) = match cpu_baseline {
        Some(b) => {
            let du = cpu_data.user_ms - b.user_ms;
            let ds = cpu_data.system_ms - b.system_ms;
            let iv = (now - b.sampled_at) * 1000;
            let pct = if iv > 0 {
                Some((du + ds) as f64 / iv as f64 * 100.0)
            } else {
                None
            };
            (Some(du), Some(ds), Some(iv), pct)
        }
        None => (None, None, None, None),
    };

    // Extra deltas
    let extra_baseline = state.extra_baselines.get(&process_db_id);
    let (delta_faults, delta_cow, delta_pageins) = match extra_baseline {
        Some(b) => (
            Some(thread_data.faults_total - b.faults_total),
            Some(thread_data.faults_cow - b.faults_cow),
            Some(thread_data.pageins - b.pageins),
        ),
        None => (None, None, None),
    };
    let (delta_bytes_read, delta_bytes_written, delta_logical_writes) = match extra_baseline {
        Some(b) => (
            Some(io_data.bytes_read - b.bytes_read),
            Some(io_data.bytes_written - b.bytes_written),
            Some(io_data.logical_writes - b.logical_writes),
        ),
        None => (None, None, None),
    };

    // ── Single transaction — all or nothing ───────────────────────────────────
    let tx = state.conn.unchecked_transaction()?;

    let result = (|| -> anyhow::Result<()> {
        tx.execute(
            "INSERT INTO sample (process_id, sampled_at) VALUES (?1, ?2)",
            params![process_db_id, now],
        )?;
        let sample_id = tx.last_insert_rowid();

        tx.execute(
            "INSERT INTO cpu
             (sample_id, user_ms, system_ms, delta_user_ms, delta_system_ms, interval_ms, cpu_percent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sample_id,
                cpu_data.user_ms,
                cpu_data.system_ms,
                delta_user,
                delta_system,
                interval_ms,
                cpu_percent,
            ],
        )?;

        tx.execute(
            "INSERT INTO memory
             (sample_id, virt_size, resident_size, resident_size_peak,
              phys_footprint, phys_footprint_peak, private_size, shared_size, swapped_size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                sample_id,
                mem_data.virt_size,
                mem_data.resident_size,
                mem_data.resident_size_peak,
                mem_data.phys_footprint,
                mem_data.phys_footprint_peak,
                mem_data.private_size,
                mem_data.shared_size,
                mem_data.swapped_size,
            ],
        )?;

        for region in &regions {
            tx.execute(
                "INSERT INTO vm_region
                 (sample_id, region_type, region_count, block_count,
                  virtual_size, resident_size, dirty_size, swapped_size,
                  shared_size, private_size, protection, share_mode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    sample_id,
                    region.region_type,
                    region.region_count,
                    region.block_count,
                    region.virtual_size,
                    region.resident_size,
                    region.dirty_size,
                    region.swapped_size,
                    region.shared_size,
                    region.private_size,
                    region.protection,
                    region.share_mode,
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO threads
             (sample_id, thread_count, running_count, faults_total, faults_cow, pageins,
              delta_faults, delta_cow, delta_pageins)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                sample_id,
                thread_data.thread_count,
                thread_data.running_count,
                thread_data.faults_total,
                thread_data.faults_cow,
                thread_data.pageins,
                delta_faults,
                delta_cow,
                delta_pageins,
            ],
        )?;

        tx.execute(
            "INSERT INTO io
             (sample_id, bytes_read, bytes_written, logical_writes,
              delta_bytes_read, delta_bytes_written, delta_logical_writes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sample_id,
                io_data.bytes_read,
                io_data.bytes_written,
                io_data.logical_writes,
                delta_bytes_read,
                delta_bytes_written,
                delta_logical_writes,
            ],
        )?;

        tx.execute(
            "INSERT INTO ports (sample_id, mach_port_count, fd_count)
             VALUES (?1, ?2, ?3)",
            params![sample_id, port_data.mach_port_count, port_data.fd_count],
        )?;

        for &display_snapshot_id in display_ids {
            tx.execute(
                "INSERT INTO sample_display (sample_id, display_snapshot_id)
                 VALUES (?1, ?2)",
                params![sample_id, display_snapshot_id],
            )?;
        }

        Ok(())
    })();

    match result {
        Ok(()) => {
            tx.commit()?;
            state.cpu_baselines.insert(
                process_db_id,
                CpuBaseline {
                    user_ms: cpu_data.user_ms,
                    system_ms: cpu_data.system_ms,
                    sampled_at: now,
                },
            );
            state.extra_baselines.insert(
                process_db_id,
                ExtraBaseline {
                    faults_total: thread_data.faults_total,
                    faults_cow: thread_data.faults_cow,
                    pageins: thread_data.pageins,
                    bytes_read: io_data.bytes_read,
                    bytes_written: io_data.bytes_written,
                    logical_writes: io_data.logical_writes,
                },
            );
            info!("pid {pid}: snapshot committed");
        }
        Err(e) => {
            tx.rollback()?;
            anyhow::bail!("transaction rolled back: {e:#}");
        }
    }

    Ok(())
}

fn bootstrap_process(
    conn: &Connection,
    pid: i32,
    start_time: i64,
    now: i64,
) -> anyhow::Result<ProcessInfo> {
    let binary_path = platform::process_binary_path(pid)?;

    let (emacs_version, configure_options, build_features) = probe_emacs_binary(&binary_path);

    conn.execute(
        "INSERT OR IGNORE INTO process
         (pid, start_time, binary_path, emacs_version, configure_options, build_features,
          first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            pid,
            start_time,
            binary_path.to_string_lossy().as_ref(),
            emacs_version,
            configure_options,
            build_features,
            now,
            now,
        ],
    )?;

    let db_id = conn.query_row(
        "SELECT id FROM process WHERE pid = ?1 AND start_time = ?2",
        params![pid, start_time],
        |r| r.get(0),
    )?;

    info!("bootstrapped pid {pid} → db id {db_id}");

    Ok(ProcessInfo { db_id, binary_path })
}

fn probe_emacs_binary(path: &PathBuf) -> (Option<String>, Option<String>, Option<String>) {
    let run = |arg: &str| -> Option<String> {
        std::process::Command::new(path)
            .args(["--batch", "--eval", arg])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };

    let version = run("(princ (emacs-version))");
    let configure = run("(princ system-configuration-options)");
    let features = run("(princ system-configuration-features)");

    (version, configure, features)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
