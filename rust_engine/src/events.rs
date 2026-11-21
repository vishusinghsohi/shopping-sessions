use crate::csv_parser::parse_line;
use crate::identity::IdentityMap;
use crate::timestamp::{ParsedTimestamp, parse_timestamp};

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    View,
    Cart,
    Purchase,
}

#[derive(Debug)]
pub struct Event {
    pub user_id: String,
    pub timestamp_ms: i64,
    pub event_type: EventType,
    pub product_id: String,
    pub revenue_cents: i64,
}

// Store one raw CSV row instead of seven separately
// allocated field buffers to reduce memory usage.
struct Candidate {
    raw_line: Vec<u8>,
    ingest_ms: i64,
}

// Convert supported timestamp formats into exact
// integer milliseconds.
fn millis(raw: &[u8]) -> Option<i64> {
    match parse_timestamp(raw)? {
        ParsedTimestamp::Millis(value) => Some(value),
        ParsedTimestamp::BeyondI64 => None,
    }
}

// Validate exact integer prices in cents.
fn price_cents(raw: &[u8]) -> Option<i64> {
    if raw.is_empty() {
        return None;
    }

    let mut value = 0i64;

    for &digit in raw {
        if !digit.is_ascii_digit() {
            return None;
        }

        value = value.checked_mul(10)?.checked_add((digit - b'0') as i64)?;

        if value > 10_000_000 {
            return None;
        }
    }

    (value > 0).then_some(value)
}

// Deduplicate globally by event_id.
//
// IMPORTANT:
// Keep the record with the earliest ingest_ts.
// Do this BEFORE checking its event timestamp,
// user, product, event type, or price.
fn choose_earliest(selected: &mut HashMap<Vec<u8>, Candidate>, fields: &[&[u8]], raw_line: &[u8]) {
    if fields.len() != 7 || fields[0].is_empty() {
        return;
    }

    // The competition guarantees valid F1 ingest_ts
    // for structurally valid event records.
    let Some(ingest_ms) = millis(fields[3]) else {
        return;
    };

    // If an earlier or equal record already exists,
    // retain it. Equal-ingest duplicates are identical.
    if selected
        .get(fields[0])
        .is_some_and(|old| old.ingest_ms <= ingest_ms)
    {
        return;
    }

    selected.insert(
        fields[0].to_vec(),
        Candidate {
            raw_line: raw_line.to_vec(),
            ingest_ms,
        },
    );
}

// Validate the winning record only.
fn validate(
    candidate: &Candidate,
    identities: &mut IdentityMap,
    window_start: i64,
    window_end: i64,
) -> Option<Event> {
    let fields = parse_line(&candidate.raw_line, 7)?;

    let user = std::str::from_utf8(fields[1]).ok()?;

    if user.is_empty() {
        return None;
    }

    let timestamp_ms = millis(fields[2])?;

    // [window_start, window_end)
    // Start is inclusive, end is exclusive.
    if timestamp_ms < window_start || timestamp_ms >= window_end {
        return None;
    }

    // Exactly 24 hours late is valid.
    // More than 24 hours late is invalid.
    //
    // i128 avoids integer overflow in subtraction.
    let lateness = candidate.ingest_ms as i128 - timestamp_ms as i128;

    if lateness > 86_400_000 {
        return None;
    }

    // Case-sensitive event types.
    let event_type = match fields[4] {
        b"view" => EventType::View,
        b"cart" => EventType::Cart,
        b"purchase" => EventType::Purchase,
        _ => return None,
    };

    let product = std::str::from_utf8(fields[5]).ok()?;

    if product.is_empty() {
        return None;
    }

    // Price is relevant only for purchases.
    let revenue_cents = match event_type {
        EventType::Purchase => price_cents(fields[6])?,
        EventType::View | EventType::Cart => 0,
    };

    // Apply the identity map after validation.
    let canonical_user = identities.canonical(user);

    Some(Event {
        user_id: canonical_user,
        timestamp_ms,
        event_type,
        product_id: product.to_owned(),
        revenue_cents,
    })
}

// Read each dataset's exact UTC window.
fn manifest_window(data_dir: &Path) -> io::Result<(i64, i64)> {
    let path = data_dir.join("manifest.json");
    let content = fs::read(path)?;

    let manifest: serde_json::Value = serde_json::from_slice(&content).map_err(io::Error::other)?;

    let bound = |key: &str| -> io::Result<i64> {
        let value = manifest[key].as_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing manifest field: {key}"),
            )
        })?;

        millis(value.as_bytes()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid timestamp in manifest: {key}"),
            )
        })
    };

    let start = bound("window_start")?;
    let end = bound("window_end")?;

    if start >= end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Manifest window_end must be after window_start",
        ));
    }

    Ok((start, end))
}

// Main event-loading function.
pub fn load_events(data_dir: &Path, identities: &mut IdentityMap) -> io::Result<Vec<Event>> {
    let (window_start, window_end) = manifest_window(data_dir)?;

    let mut files = fs::read_dir(data_dir.join("events"))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;

    // Read all CSV partitions.
    files.retain(|p| p.extension().is_some_and(|e| e == "csv"));

    files.sort();

    let mut selected: HashMap<Vec<u8>, Candidate> = HashMap::new();

    let mut structural_rows = 0usize;
    let mut malformed_rows = 0usize;

    println!("\nEVENT INGESTION");
    println!("  Event CSV files: {}", files.len());

    for path in files {
        let file = File::open(&path)?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();

        // Skip exactly one physical header per file.
        reader.read_until(b'\n', &mut line)?;

        loop {
            line.clear();

            let bytes_read = reader.read_until(b'\n', &mut line)?;

            if bytes_read == 0 {
                break;
            }

            // LF ends a record.
            // CR remains part of the last field.
            if line.last() == Some(&b'\n') {
                line.pop();
            }

            match parse_line(&line, 7) {
                Some(fields) => {
                    structural_rows += 1;

                    choose_earliest(&mut selected, &fields, &line);
                }
                None => {
                    malformed_rows += 1;
                }
            }
        }
    }

    let unique_events = selected.len();

    println!("  Structurally valid rows: {structural_rows}");
    println!("  Malformed rows: {malformed_rows}");
    println!("  Unique event IDs: {unique_events}");

    // Validate only after global deduplication.
    let mut events = Vec::new();

    for candidate in selected.into_values() {
        if let Some(event) = validate(&candidate, identities, window_start, window_end) {
            events.push(event);
        }
    }

    // Sort by canonical user and timestamp.
    // This prepares events for sessionization.
    events.sort_unstable_by(|a, b| {
        a.user_id
            .as_bytes()
            .cmp(b.user_id.as_bytes())
            .then(a.timestamp_ms.cmp(&b.timestamp_ms))
    });

    let purchase_count = events
        .iter()
        .filter(|event| event.event_type == EventType::Purchase)
        .count();

    let revenue_cents: i64 = events.iter().map(|event| event.revenue_cents).sum();

    println!("\nEVENT VALIDATION");
    println!("  Valid events: {}", events.len());
    println!("  Valid purchases: {purchase_count}");
    println!("  Revenue (cents): {revenue_cents}");

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_window() -> (i64, i64) {
        (
            millis(b"2026-10-01T00:00:00Z").unwrap(),
            millis(b"2026-10-15T00:00:00Z").unwrap(),
        )
    }

    fn candidate(raw: &[u8]) -> Candidate {
        let fields = parse_line(raw, 7).unwrap();

        Candidate {
            raw_line: raw.to_vec(),
            ingest_ms: millis(fields[3]).unwrap(),
        }
    }

    #[test]
    fn earlier_invalid_duplicate_wins() {
        let mut selected = HashMap::new();

        let later = b"e2,u1,2026-10-03T10:00:00Z,\
2026-10-03T10:05:00Z,view,p1,";

        let earlier = b"e2,u1,2026-10-03 10:00:00,\
2026-10-03T10:01:00Z,view,p1,";

        let later_fields = parse_line(later, 7).unwrap();
        let earlier_fields = parse_line(earlier, 7).unwrap();

        choose_earliest(&mut selected, &later_fields, later);

        choose_earliest(&mut selected, &earlier_fields, earlier);

        assert_eq!(selected.len(), 1);

        let (start, end) = test_window();
        let mut identities = IdentityMap::new();

        let winner = selected.get(b"e2".as_slice()).unwrap();

        // The earlier record has an invalid timestamp.
        // The later valid duplicate cannot rescue it.
        assert!(validate(winner, &mut identities, start, end,).is_none());
    }

    #[test]
    fn price_validation_is_exact() {
        assert_eq!(price_cents(b"00000125"), Some(125));
        assert_eq!(price_cents(b"10000000"), Some(10_000_000));

        assert_eq!(price_cents(b"10000001"), None);
        assert_eq!(price_cents(b"0"), None);
        assert_eq!(price_cents(b"-125"), None);
        assert_eq!(price_cents(b"12.50"), None);
        assert_eq!(price_cents(b""), None);
        assert_eq!(price_cents("١٢٥".as_bytes()), None);
    }

    #[test]
    fn lateness_boundary_is_exact() {
        let (start, end) = test_window();
        let mut identities = IdentityMap::new();

        let exactly_24h = candidate(
            b"e1,u1,2026-10-02T00:00:00Z,\
2026-10-03T00:00:00Z,purchase,p1,2500",
        );

        assert!(validate(&exactly_24h, &mut identities, start, end,).is_some());

        let one_second_late = candidate(
            b"e1,u1,2026-10-02T00:00:00Z,\
2026-10-03T00:00:01Z,purchase,p1,2500",
        );

        assert!(validate(&one_second_late, &mut identities, start, end,).is_none());
    }

    #[test]
    fn enforces_half_open_event_window() {
        let (start, end) = test_window();
        let mut identities = IdentityMap::new();

        let at_start = candidate(
            b"e1,u1,2026-10-01T00:00:00Z,\
2026-10-01T00:01:00Z,view,p1,",
        );

        assert!(validate(&at_start, &mut identities, start, end,).is_some());

        let at_end = candidate(
            b"e2,u1,2026-10-15T00:00:00Z,\
2026-10-15T00:01:00Z,view,p1,",
        );

        assert!(validate(&at_end, &mut identities, start, end,).is_none());
    }

    #[test]
    fn views_and_carts_ignore_prices() {
        let (start, end) = test_window();
        let mut identities = IdentityMap::new();

        let view = candidate(
            b"e1,u1,2026-10-03T10:00:00Z,\
2026-10-03T10:01:00Z,view,p1,invalid",
        );

        let validated = validate(&view, &mut identities, start, end).unwrap();

        assert_eq!(validated.revenue_cents, 0);

        let cart = candidate(
            b"e2,u1,2026-10-03T10:00:00Z,\
2026-10-03T10:01:00Z,cart,p1,",
        );

        assert!(validate(&cart, &mut identities, start, end,).is_some());
    }
}
