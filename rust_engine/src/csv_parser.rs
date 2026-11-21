
pub fn parse_line(
    line: &[u8],
    expected_columns: usize,
) -> Option<Vec<&[u8]>> {
    if line.is_empty() {
        return None;
    }

    let mut fields = Vec::with_capacity(expected_columns);

    for part in line.split(|&b| b == b',') {
        let value = if part.first() == Some(&b'"') {
            // Quoted fields must have a closing quote.
            if part.len() < 2 || part.last() != Some(&b'"') {
                return None;
            }

            let inner = &part[1..part.len() - 1];

            if inner.contains(&b'"') {
                return None;
            }

            inner
        } else {
            // Quotes inside unquoted fields are invalid.
            if part.contains(&b'"') {
                return None;
            }

            part
        };

        fields.push(value);
    }

    if fields.len() != expected_columns {
        return None;
    }

    Some(fields)
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn preserves_spaces_and_carriage_returns() {
        let row = parse_line(b"e1,\" u1\",1000\r", 3).unwrap();

        assert_eq!(row[0], b"e1");
        assert_eq!(row[1], b" u1");
        assert_eq!(row[2], b"1000\r");
    }

    #[test]
    fn handles_quoted_empty_values() {
        let row = parse_line(b"\"\",x", 2).unwrap();
        assert_eq!(row[0], b"");
    }

    #[test]
    fn rejects_malformed_rows() {
        assert!(parse_line(b"", 2).is_none());
        assert!(parse_line(b"u1", 2).is_none());
        assert!(parse_line(b"u1,\"bad\"extra", 2).is_none());
        assert!(parse_line(b"u1,\"bad", 2).is_none());
        assert!(parse_line(b"u1,\"a,b\"", 2).is_none());
    }
}