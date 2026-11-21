use crate::csv_parser::parse_line;
use crate::identity::IdentityMap;
use crate::timestamp::{ParsedTimestamp, parse_timestamp};

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;

const SEVEN_DAYS_MS: i128 = 604_800_000;

#[derive(Debug)]
pub struct Click {
    pub click_id: String,
    pub user_id: String,
    pub campaign_id: String,
    pub timestamp_ms: i64,
}

pub struct ClickData {
    pub clicks: Vec<Click>,
    pub channels: HashMap<String, String>,
}

// All timestamps are integer milliseconds.
fn millis(raw: &[u8]) -> Option<i64> {
    match parse_timestamp(raw)? {
        ParsedTimestamp::Millis(value) => Some(value),
        ParsedTimestamp::BeyondI64 => None,
    }
}

// Read the campaign lookup table.
fn load_campaigns(data_dir: &Path) -> io::Result<HashMap<String, String>> {
    let file = File::open(data_dir.join("campaigns.csv"))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut channels = HashMap::new();

    // Skip exactly one header.
    reader.read_until(b'\n', &mut line)?;

    loop {
        line.clear();

        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }

        // Preserve carriage returns as data.
        if line.last() == Some(&b'\n') {
            line.pop();
        }

        let fields = parse_line(&line, 2).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Malformed campaigns.csv row")
        })?;

        let campaign_id = std::str::from_utf8(fields[0]).map_err(io::Error::other)?;

        let channel = std::str::from_utf8(fields[1]).map_err(io::Error::other)?;

        channels.insert(campaign_id.to_owned(), channel.to_owned());
    }

    Ok(channels)
}

// Read the fixture's actual window.
fn manifest_window(data_dir: &Path) -> io::Result<(i64, i64)> {
    let content = fs::read(data_dir.join("manifest.json"))?;

    let manifest: serde_json::Value = serde_json::from_slice(&content).map_err(io::Error::other)?;

    let bound = |key: &str| -> io::Result<i64> {
        let raw = manifest[key].as_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing manifest field: {key}"),
            )
        })?;

        millis(raw.as_bytes()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid manifest timestamp: {key}"),
            )
        })
    };

    let start = bound("window_start")?;
    let end = bound("window_end")?;

    if start >= end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid manifest window",
        ));
    }

    Ok((start, end))
}

// Validate a click after global click_id deduplication.
fn validate_click(
    fields: &[&[u8]],
    identities: &mut IdentityMap,
    channels: &HashMap<String, String>,
    window_start: i64,
    window_end: i64,
) -> Option<Click> {
    if fields.len() != 4 || fields[0].is_empty() {
        return None;
    }

    let click_id = std::str::from_utf8(fields[0]).ok()?;
    let user_id = std::str::from_utf8(fields[1]).ok()?;
    let campaign_id = std::str::from_utf8(fields[2]).ok()?;

    if user_id.is_empty() || !channels.contains_key(campaign_id) {
        return None;
    }

    let timestamp_ms = millis(fields[3])?;

    // Valid click window:
    // [window_start - 7 days, window_end)
    //
    // i128 avoids subtraction overflow.
    let earliest = window_start as i128 - SEVEN_DAYS_MS;

    if (timestamp_ms as i128) < earliest || timestamp_ms >= window_end {
        return None;
    }

    Some(Click {
        click_id: click_id.to_owned(),
        user_id: identities.canonical(user_id),
        campaign_id: campaign_id.to_owned(),
        timestamp_ms,
    })
}

fn first_occurrence(seen: &mut HashSet<Vec<u8>>, click_id: &[u8]) -> bool {
    if click_id.is_empty() {
        return false;
    }

    seen.insert(click_id.to_vec())
}

// Read, deduplicate and validate every click partition.
pub fn load_clicks(data_dir: &Path, identities: &mut IdentityMap) -> io::Result<ClickData> {
    let (window_start, window_end) = manifest_window(data_dir)?;

    let channels = load_campaigns(data_dir)?;

    let mut files = fs::read_dir(data_dir.join("clicks"))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;

    files.retain(|p| p.extension().is_some_and(|e| e == "csv"));

    files.sort();

    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut clicks = Vec::new();

    let mut structural_rows = 0usize;
    let mut malformed_rows = 0usize;
    let mut duplicate_ids = 0usize;

    println!("\nCLICK INGESTION");
    println!("  Click CSV files: {}", files.len());

    for path in files {
        let mut reader = BufReader::new(File::open(path)?);
        let mut line = Vec::new();

        // Skip exactly one header per CSV file.
        reader.read_until(b'\n', &mut line)?;

        loop {
            line.clear();

            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }

            // Remove LF only, not CR.
            if line.last() == Some(&b'\n') {
                line.pop();
            }

            let Some(fields) = parse_line(&line, 4) else {
                malformed_rows += 1;
                continue;
            };

            structural_rows += 1;

            if fields[0].is_empty() {
                continue;
            }

            // Deduplicate globally by click_id.
            // Competition duplicate rows are identical.
            if !first_occurrence(&mut seen, fields[0]) {
                duplicate_ids += 1;
                continue;
            }

            if let Some(click) =
                validate_click(&fields, identities, &channels, window_start, window_end)
            {
                clicks.push(click);
            }
        }
    }

    // Attribution requires canonical user/time ordering.
    // Within equal timestamps, smallest click_id first.
    clicks.sort_unstable_by(|a, b| {
        a.user_id
            .as_bytes()
            .cmp(b.user_id.as_bytes())
            .then(a.timestamp_ms.cmp(&b.timestamp_ms))
            .then(a.click_id.as_bytes().cmp(b.click_id.as_bytes()))
    });

    println!("  Campaigns loaded: {}", channels.len());
    println!("  Structurally valid rows: {structural_rows}");
    println!("  Malformed rows: {malformed_rows}");
    println!("  Duplicate click IDs: {duplicate_ids}");
    println!("  Unique nonempty click IDs: {}", seen.len());
    println!("  Valid clicks: {}", clicks.len());

    Ok(ClickData { clicks, channels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (HashMap<String, String>, i64, i64) {
        let channels = HashMap::from([
            ("c_1".to_string(), "search".to_string()),
            ("c_2".to_string(), "social".to_string()),
        ]);

        let start = millis(b"2026-10-01T00:00:00Z").unwrap();
        let end = millis(b"2026-10-15T00:00:00Z").unwrap();

        (channels, start, end)
    }

    #[test]
    fn seven_day_boundary_is_inclusive() {
        let (channels, start, end) = setup();
        let mut identities = IdentityMap::new();

        let valid: [&[u8]; 4] = [b"k1", b"u1", b"c_1", b"2026-09-24T00:00:00Z"];

        assert!(validate_click(&valid, &mut identities, &channels, start, end).is_some());

        let invalid: [&[u8]; 4] = [b"k2", b"u1", b"c_1", b"1790207999999"];

        assert!(validate_click(&invalid, &mut identities, &channels, start, end).is_none());
    }

    #[test]
    fn click_window_end_is_exclusive() {
        let (channels, start, end) = setup();
        let mut identities = IdentityMap::new();

        let at_end: [&[u8]; 4] = [b"k1", b"u1", b"c_1", b"2026-10-15T00:00:00Z"];

        assert!(validate_click(&at_end, &mut identities, &channels, start, end).is_none());

        let before_end: [&[u8]; 4] = [b"k2", b"u1", b"c_1", b"2026-10-14T23:59:59Z"];

        assert!(validate_click(&before_end, &mut identities, &channels, start, end).is_some());
    }

    #[test]
    fn rejects_unknown_campaigns_and_bad_timestamps() {
        let (channels, start, end) = setup();
        let mut identities = IdentityMap::new();

        let unknown: [&[u8]; 4] = [b"k1", b"u1", b"c_999", b"2026-10-03T09:00:00Z"];

        assert!(validate_click(&unknown, &mut identities, &channels, start, end).is_none());

        let bad_timestamp: [&[u8]; 4] = [b"k2", b"u1", b"c_1", b"2026-10-03 09:00:00"];

        assert!(validate_click(&bad_timestamp, &mut identities, &channels, start, end).is_none());
    }

    #[test]
    fn applies_transitive_identity_mapping() {
        let (channels, start, end) = setup();
        let mut identities = IdentityMap::new();

        identities.union("u3", "u9");
        identities.union("u9", "U7");

        let row: [&[u8]; 4] = [b"k1", b"u3", b"c_1", b"2026-10-03T09:00:00Z"];

        let click = validate_click(&row, &mut identities, &channels, start, end).unwrap();

        assert_eq!(click.user_id, "U7");
        assert_eq!(click.campaign_id, "c_1");
    }

    #[test]
    fn deduplicates_click_ids_exactly() {
        let mut seen = HashSet::new();

        assert!(first_occurrence(&mut seen, b"k1"));
        assert!(!first_occurrence(&mut seen, b"k1"));

        // Case and spaces are significant.
        assert!(first_occurrence(&mut seen, b"K1"));
        assert!(first_occurrence(&mut seen, b" k1"));

        assert!(!first_occurrence(&mut seen, b""));
        assert_eq!(seen.len(), 3);
    }
}
