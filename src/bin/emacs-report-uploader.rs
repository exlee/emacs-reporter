// src/bin/emacs-report-uploader.rs

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// Baked in at compile time via environment variable
const UPLOAD_URL: &str = match option_env!("EMACS_REPORTER_UPLOAD_URL") {
        Some(url) => url,
            None => "http://localhost:9999",
};

// SQLite3 magic header bytes
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

// 50 MB ceiling — generous but prevents accidents
const MAX_SIZE_BYTES: u64 = 50 * 1024 * 1024;

fn main() -> anyhow::Result<()> {
    let db_path = resolve_db_path()?;

    println!("found: {}", db_path.display());

    validate_sqlite(&db_path)?;
    validate_size(&db_path)?;

    let Ok(user_hash) = read_user_hash(&db_path) else {
        println!("error: no meta.user_hash in database, exiting");
        return Ok(());
    };

    println!("compressing...");
    let compressed = compress(&db_path)?;
    let compressed_kb = compressed.len() / 1024;
    println!("compressed: {compressed_kb} KB");

    let filename = format!("{}.db.bz2", user_hash);
    println!("uploading as {filename}...");

    upload(&compressed, &filename)?;

    println!("done — thank you for contributing!");
    Ok(())
}

// ── Path resolution ───────────────────────────────────────────────────────────

fn resolve_db_path() -> anyhow::Result<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from("emacs_reporter.db")
    };

    anyhow::ensure!(
        path.exists(),
        "database not found: {}\nhint: run from the directory containing emacs_reporter.db \
         or pass the path as an argument",
        path.display()
    );

    Ok(path)
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_sqlite(path: &Path) -> anyhow::Result<()> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 16];
    f.read_exact(&mut magic)
        .map_err(|_| anyhow::anyhow!("file too small to be a SQLite database"))?;

    anyhow::ensure!(
        magic == SQLITE_MAGIC,
        "file does not appear to be a SQLite3 database (magic header mismatch)"
    );

    Ok(())
}

fn validate_size(path: &Path) -> anyhow::Result<()> {
    let size = std::fs::metadata(path)?.len();
    anyhow::ensure!(
        size <= MAX_SIZE_BYTES,
        "database is too large ({} MB, limit is {} MB)",
        size / 1024 / 1024,
        MAX_SIZE_BYTES / 1024 / 1024
    );
    Ok(())
}

// ── Compression ───────────────────────────────────────────────────────────────

fn compress(path: &Path) -> anyhow::Result<Vec<u8>> {
    let data = std::fs::read(path)?;
    let output = Vec::new();

    // level 9 = maximum compression
    let mut encoder = bzip2::write::BzEncoder::new(output, bzip2::Compression::best());
    encoder.write_all(&data)?;
    Ok(encoder.finish()?)
}

// ── Upload ────────────────────────────────────────────────────────────────────

fn upload(data: &[u8], filename: &str) -> anyhow::Result<()> {
    let url = format!("{}/{}", UPLOAD_URL.trim_end_matches('/'), filename);

    let response = ureq::put(&url)
        .header("Content-Type", "application/x-bzip2")
        .header("Content-Length", &data.len().to_string())
        .send(data)
        .map_err(|e| anyhow::anyhow!("upload failed: {e}"))?;

    anyhow::ensure!(
        response.status() == 200,
        "upload rejected by server: HTTP {}",
        response.status()
    );

    Ok(())
}

// read user_hash from the database before compressing
fn read_user_hash(path: &Path) -> anyhow::Result<String> {
    let conn = rusqlite::Connection::open(path)?;
    let hash: String = conn.query_row(
        "SELECT user_hash FROM meta WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(hash)
}
