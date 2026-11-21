
mod csv_parser;
mod timestamp;

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;

fn inspect_folder(
    data_dir: &Path,
    folder: &str,
    expected_columns: usize,
) -> io::Result<()> {
    let directory = data_dir.join(folder);

    let mut files = fs::read_dir(&directory)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;

    // Only read CSV files.
    files.retain(|p| p.extension().is_some_and(|e| e == "csv"));
    files.sort();

    let mut valid = 0usize;
    let mut malformed = 0usize;

    for path in &files {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut first_line = true;

        loop {
            line.clear();

            // Read until LF, without removing carriage returns.
            let bytes_read = reader.read_until(b'\n', &mut line)?;

            if bytes_read == 0 {
                break;
            }

            if line.last() == Some(&b'\n') {
                line.pop();
            }

            // Every CSV file has its own header.
            if first_line {
                first_line = false;
                continue;
            }

            match csv_parser::parse_line(&line, expected_columns) {
                Some(_) => valid += 1,
                None => malformed += 1,
            }
        }
    }

    println!("{}:", folder);
    println!("  CSV files: {}", files.len());
    println!("  Structurally valid rows: {}", valid);
    println!("  Malformed rows: {}", malformed);

    Ok(())
}

fn main() -> io::Result<()> {
    let data_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fixtures/E1/input".to_string());

    let data_dir = Path::new(&data_path);

    println!("SHOPPING SESSIONS - RUST CSV READER");
    println!("Input: {}\n", data_dir.display());

    inspect_folder(data_dir, "events", 7)?;
    inspect_folder(data_dir, "clicks", 4)?;
    inspect_folder(data_dir, "links", 2)?;

    Ok(())
}