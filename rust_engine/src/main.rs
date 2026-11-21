mod csv_parser;
mod identity;
mod timestamp;

use identity::IdentityMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;

fn csv_files(directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut files = fs::read_dir(directory)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;

    files.retain(|p| p.extension().is_some_and(|e| e == "csv"));
    files.sort();

    Ok(files)
}

fn load_links(data_dir: &Path) -> io::Result<IdentityMap> {
    let mut identities = IdentityMap::new();
    let mut accepted = 0usize;
    let mut dropped = 0usize;

    for path in csv_files(&data_dir.join("links"))? {
        let mut reader = BufReader::new(File::open(path)?);
        let mut line = Vec::new();

        // Skip exactly the first physical row (header).
        reader.read_until(b'\n', &mut line)?;

        loop {
            line.clear();

            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }

            // Remove LF only. Preserve CR as data.
            if line.last() == Some(&b'\n') {
                line.pop();
            }

            let Some(fields) = csv_parser::parse_line(&line, 2) else {
                dropped += 1;
                continue;
            };

            if fields[0].is_empty() || fields[1].is_empty() {
                dropped += 1;
                continue;
            }

            let a = std::str::from_utf8(fields[0]).expect("Input must be UTF-8");
            let b = std::str::from_utf8(fields[1]).expect("Input must be UTF-8");

            identities.union(a, b);
            accepted += 1;
        }
    }

    println!("IDENTITY RESOLUTION");
    println!("  Accepted links: {accepted}");
    println!("  Dropped links: {dropped}");

    for user in ["u1", "u2", "u3", "u9", "u4", "user_id_a"] {
        println!("  {:?} -> {:?}", user, identities.canonical(user));
    }

    Ok(identities)
}

fn inspect_folder(data_dir: &Path, folder: &str, expected_columns: usize) -> io::Result<()> {
    let files = csv_files(&data_dir.join(folder))?;
    let mut valid = 0usize;
    let mut malformed = 0usize;

    for path in &files {
        let mut reader = BufReader::new(File::open(path)?);
        let mut line = Vec::new();

        // Skip the header of each file.
        reader.read_until(b'\n', &mut line)?;

        loop {
            line.clear();

            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }

            if line.last() == Some(&b'\n') {
                line.pop();
            }

            if csv_parser::parse_line(&line, expected_columns).is_some() {
                valid += 1;
            } else {
                malformed += 1;
            }
        }
    }

    println!("{folder}:");
    println!("  CSV files: {}", files.len());
    println!("  Structurally valid rows: {valid}");
    println!("  Malformed rows: {malformed}");

    Ok(())
}

fn main() -> io::Result<()> {
    let input = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/L1/input".to_string());

    let data_dir = Path::new(&input);

    println!("SHOPPING SESSIONS - RUST ENGINE");
    println!("Input: {}\n", data_dir.display());

    // Links must be resolved before processing events and clicks.
    let _identities = load_links(data_dir)?;

    println!();
    inspect_folder(data_dir, "events", 7)?;
    inspect_folder(data_dir, "clicks", 4)?;
    inspect_folder(data_dir, "links", 2)?;

    Ok(())
}
