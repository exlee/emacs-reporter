// src/bin/emacs-report-merge.rs

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::Connection;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: emacs-report-merge <directory-of-db.bz2-files>");
        std::process::exit(1);
    }

    let input_dir = PathBuf::from(&args[1]);
    anyhow::ensure!(input_dir.is_dir(), "not a directory: {}", input_dir.display());

    let output_path = resolve_output_path(&input_dir)?;
    println!("working database: {}", output_path.display());

    let mut files: Vec<PathBuf> = fs::read_dir(&input_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bz2"))
        .collect();
    files.sort();

    if files.is_empty() {
        anyhow::bail!("no .bz2 files found in {}", input_dir.display());
    }

    println!("found {} file(s)", files.len());

    let error_log_path = input_dir.join("error.log");
    let mut error_count = 0;

    for file in &files {
        let filename = file.file_name().unwrap_or_default().to_string_lossy().to_string();
        print!("merging {filename}... ");
        std::io::stdout().flush().ok();

        match merge_file(file, &output_path) {
            Ok(rows) => println!("{rows} rows inserted"),
            Err(e) => {
                println!("FAILED");
                let msg = format!("[{}] {}: {:#}\n", timestamp(), filename, e);
                eprintln!("{}", msg.trim());

                // copy failed source file for inspection
                let error_copy = input_dir.join(format!("error_{filename}"));
                if let Err(ce) = fs::copy(file, &error_copy) {
                    eprintln!("  (could not copy to {}: {ce})", error_copy.display());
                }

                // append to error log
                let mut log = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&error_log_path)
                    .unwrap();
                let _ = log.write_all(msg.as_bytes());

                error_count += 1;
            }
        }
    }

    println!(
        "\ndone — {} file(s) processed, {} error(s)",
        files.len(),
        error_count
    );
    if error_count > 0 {
        println!("see {}", error_log_path.display());
    }

    Ok(())
}

// ── Output path resolution ────────────────────────────────────────────────────

fn resolve_output_path(input_dir: &Path) -> anyhow::Result<PathBuf> {
    // 1. all_reports.db in cwd
    let cwd_path = PathBuf::from("all_reports.db");
    if cwd_path.exists() {
        return Ok(cwd_path);
    }

    // 2. emacs_reporter.db in cwd
    let reporter_path = PathBuf::from("emacs_reporter.db");
    if reporter_path.exists() {
        return Ok(reporter_path);
    }

    // 3. clone the first .bz2 in the directory as all_reports.db
    let mut files: Vec<PathBuf> = fs::read_dir(input_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bz2"))
        .collect();
    files.sort();

    let first = files
        .first()
        .context("no .bz2 files found to seed working database")?;

    println!("seeding working database from {}", first.display());
    let data = decompress(first)?;
    fs::write(&cwd_path, &data).context("failed to write all_reports.db")?;
    println!("created all_reports.db");

    Ok(cwd_path)
}

// ── Per-file merge ────────────────────────────────────────────────────────────

fn merge_file(path: &Path, output_path: &Path) -> anyhow::Result<u64> {
    let data = decompress(path)?;

    // write to temp file so rusqlite can open it
    let tmp_path = path.with_extension("tmp.db");
    fs::write(&tmp_path, &data)?;
    let _tmp_guard = TempFile(&tmp_path);

    let src = Connection::open(&tmp_path)?;
    let mut dst = Connection::open(output_path)?;

    let tables = user_tables(&src)?;
    let mut total_rows: u64 = 0;

    // attach source db and merge inside a single transaction on dst
    // FK checks off for duration — we're copying already-valid data
    dst.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let tx = dst.transaction()?;

    let result = (|| -> anyhow::Result<u64> {
        let mut rows_inserted: u64 = 0;

        for table in &tables {
            // skip tables that don't exist in dst yet — schema mismatch
            if !table_exists_in_dst(&tx, table)? {
                eprintln!("  skipping unknown table: {table}");
                continue;
            }

            let columns = table_columns(&src, table)?;
            if columns.is_empty() {
                continue;
            }

            let col_list = columns.join(", ");
            let placeholders = columns
                .iter()
                .map(|c| format!(":{c}"))
                .collect::<Vec<_>>()
                .join(", ");

            let select_sql = format!("SELECT {col_list} FROM \"{table}\"");
            let insert_sql = format!(
                "INSERT OR IGNORE INTO \"{table}\" ({col_list}) VALUES ({placeholders})"
            );

            let src_rows = {
                let mut stmt = src.prepare(&select_sql)?;
                let col_count = columns.len();
                let rows: Vec<Vec<rusqlite::types::Value>> = stmt
                    .query_map([], |row| {
                        (0..col_count)
                            .map(|i| row.get::<_, rusqlite::types::Value>(i))
                            .collect()
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                rows
            };

            let mut stmt = tx.prepare(&insert_sql)?;
            for row in &src_rows {
                let named: Vec<(&str, &dyn rusqlite::types::ToSql)> = columns
                    .iter()
                    .zip(row.iter())
                    .map(|(c, v)| (c.as_str(), v as &dyn rusqlite::types::ToSql))
                    .collect();
                match stmt.execute(named.as_slice()) {
                    Ok(n) => rows_inserted += n as u64,
                    Err(e) => return Err(e.into()),
                }
            }
        }

        Ok(rows_inserted)
    })();

    match result {
        Ok(n) => {
            tx.commit()?;
            dst.execute_batch("PRAGMA foreign_keys = ON;")?;
            total_rows += n;
            Ok(total_rows)
        }
        Err(e) => {
            // tx rolls back on drop
            dst.execute_batch("PRAGMA foreign_keys = ON;")?;
            Err(e)
        }
    }
}

// ── Schema helpers ────────────────────────────────────────────────────────────

fn user_tables(conn: &Connection) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master
         WHERE type = 'table'
           AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;
    let tables = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tables)
}

fn table_exists_in_dst(conn: &Connection, table: &str) -> anyhow::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        rusqlite::params![table],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn table_columns(conn: &Connection, table: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(cols)
}

// ── Decompression ─────────────────────────────────────────────────────────────

fn decompress(path: &Path) -> anyhow::Result<Vec<u8>> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut decoder = bzip2::read::BzDecoder::new(file);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .with_context(|| format!("failed to decompress {}", path.display()))?;
    Ok(out)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}")
}

// RAII temp file cleanup
struct TempFile<'a>(&'a Path);

impl Drop for TempFile<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}
