//! Reader for the INI dialect of the vendor's config files.

pub struct Entry {
    pub key: String,
    pub value: String,
    /// Text of the comment on the line right before this entry, if any.
    pub comment: Option<String>,
}

pub struct Section {
    pub name: String,
    pub entries: Vec<Entry>,
}

pub struct Ini {
    pub sections: Vec<Section>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut sections: Vec<Section> = Vec::new();
        let mut current = None;
        let mut pending_label = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                pending_label = None;
            } else if let Some(comment) = line.strip_prefix(';') {
                let comment = comment.trim();
                pending_label = (!comment.is_empty()).then(|| comment.to_string());
            } else if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                pending_label = None;
                current = Some(section_index(&mut sections, name.trim()));
            } else if let Some((key, value)) = line.split_once('=') {
                let comment = pending_label.take();
                let key = key.trim();
                if let Some(index) = current
                    && !key.is_empty()
                    && !sections[index].entries.iter().any(|e| e.key == key)
                {
                    sections[index].entries.push(Entry {
                        key: key.to_string(),
                        value: value.split(';').next().unwrap_or("").trim().to_string(),
                        comment,
                    });
                }
            } else {
                // Some files forget the semicolon on a key legend.
                pending_label = Some(line.to_string());
            }
        }
        Ini { sections }
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let entries = &self.section(section)?.entries;
        let entry = entries.iter().find(|e| e.key == key)?;
        Some(&entry.value)
    }
}

fn section_index(sections: &mut Vec<Section>, name: &str) -> usize {
    if let Some(index) = sections.iter().position(|s| s.name == name) {
        return index;
    }
    sections.push(Section {
        name: name.to_string(),
        entries: Vec::new(),
    });
    sections.len() - 1
}

/// Decodes a config file, or says why it can't.
///
/// A `FF FE` BOM means UTF-16 LE. Anything else must be UTF-8: most files are plain ASCII, and the
/// ones with non-ASCII legends and no BOM are UTF-8. Nothing is replaced, so a bad legend can't
/// reach the output.
pub fn decode(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (pairs, odd) = rest.as_chunks::<2>();
        if !odd.is_empty() {
            return Err("UTF-16 with an odd number of bytes".to_string());
        }
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        return String::from_utf16(&units).map_err(|_| "invalid UTF-16".to_string());
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| "neither UTF-16 with a BOM nor UTF-8".to_string())
}

/// Parses a decimal or `0x` hexadecimal number.
pub fn parse_num(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Parses a comma-separated list of numbers. A trailing comma is allowed.
pub fn parse_list(text: &str) -> Option<Vec<u32>> {
    let text = text.trim();
    let text = text.strip_suffix(',').unwrap_or(text);
    if text.is_empty() {
        return Some(Vec::new());
    }
    text.split(',').map(parse_num).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sections_and_skips_comments() {
        let ini = Ini::parse("[OPT]\r\n;note\r\nFw=1\r\n\r\n[KEY]\r\nK1=2\r\n");
        assert_eq!(ini.get("OPT", "Fw"), Some("1"));
        assert_eq!(ini.get("KEY", "K1"), Some("2"));
        assert_eq!(ini.get("OPT", "K1"), None);
        assert_eq!(ini.get("NOPE", "K1"), None);
        assert_eq!(ini.section("OPT").unwrap().entries.len(), 1);
    }

    #[test]
    fn repeated_keys_keep_the_first() {
        let ini = Ini::parse("[OPT]\nFw=1\nFw=2\n");
        assert_eq!(ini.get("OPT", "Fw"), Some("1"));
        assert_eq!(ini.section("OPT").unwrap().entries.len(), 1);
    }

    #[test]
    fn keys_and_values_are_trimmed() {
        let ini = Ini::parse("[OPT]\n  MatchStr  =  0x1F \t\n");
        assert_eq!(ini.get("OPT", "MatchStr"), Some("0x1F"));
    }

    #[test]
    fn inline_comments_are_not_part_of_the_value() {
        let ini = Ini::parse("[OPT]\nLedOpt1=  1,0,1 ;some note\n");
        assert_eq!(ini.get("OPT", "LedOpt1"), Some("1,0,1"));
        assert_eq!(ini.section("OPT").unwrap().entries[0].comment, None);
    }

    #[test]
    fn comment_on_the_line_before_becomes_the_label() {
        let ini = Ini::parse("[KEY]\n;Esc\nK1=1\nK2=2\n;Tab\n\nK3=3\n;=\nK4=4\n;\nK5=5\n");
        let labels: Vec<Option<&str>> = ini
            .section("KEY")
            .unwrap()
            .entries
            .iter()
            .map(|e| e.comment.as_deref())
            .collect();
        // K2 has no comment, and a blank line separates Tab from K3.
        assert_eq!(labels, [Some("Esc"), None, None, Some("="), None]);
    }

    #[test]
    fn bare_line_before_a_key_counts_as_its_label() {
        // Some files forget the semicolon on a legend.
        let ini = Ini::parse("[KEY]\nDelete\nK15=1\n");
        let entry = &ini.section("KEY").unwrap().entries[0];
        assert_eq!(entry.key, "K15");
        assert_eq!(entry.comment.as_deref(), Some("Delete"));
    }

    #[test]
    fn numbers_in_decimal_and_hex() {
        assert_eq!(parse_num("84"), Some(84));
        assert_eq!(parse_num(" 0x8 "), Some(8));
        assert_eq!(parse_num("0x0700000D"), Some(0x0700_000D));
        assert_eq!(parse_num("0x01fa"), Some(0x01FA));
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("abc"), None);
    }

    #[test]
    fn lists_allow_spaces_and_a_trailing_comma() {
        assert_eq!(parse_list("25,25, 0x02,0"), Some(vec![25, 25, 2, 0]));
        assert_eq!(parse_list("0x02,0x91,0x00,"), Some(vec![2, 0x91, 0]));
        assert_eq!(parse_list("1,,2"), None);
        assert_eq!(parse_list("1,x"), None);
    }

    #[test]
    fn a_section_line_clears_a_pending_label() {
        let ini = Ini::parse("[KEY]\n;Note\n[FN1]\nK1=1\n");
        assert_eq!(ini.section("FN1").unwrap().entries[0].comment, None);
    }

    fn utf16_le(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decodes_utf16_when_the_bom_says_so() {
        assert_eq!(decode(&utf16_le("[OPT]\r\nFw=1")).unwrap(), "[OPT]\r\nFw=1");
    }

    #[test]
    fn decodes_files_without_a_bom_as_utf8() {
        assert_eq!(decode(b"[OPT]\nFw=1").unwrap(), "[OPT]\nFw=1");
        let text = ";\u{97f3}\u{91cf}-";
        assert_eq!(decode(text.as_bytes()).unwrap(), text);
    }

    #[test]
    fn drops_a_utf8_bom() {
        assert_eq!(decode(b"\xEF\xBB\xBF[OPT]").unwrap(), "[OPT]");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_an_error() {
        assert!(decode(b";\xC9\xCF\nFw=1").is_err());
    }

    #[test]
    fn a_utf16_be_bom_is_an_error() {
        assert!(decode(b"\xFE\xFF\x00[\x00O").is_err());
    }

    #[test]
    fn a_lone_surrogate_is_an_error() {
        assert!(decode(b"\xFF\xFE\x00\xD8").is_err());
    }

    #[test]
    fn utf16_with_an_odd_number_of_bytes_is_an_error() {
        let mut bytes = utf16_le("[OPT]");
        bytes.push(b'x');
        assert!(decode(&bytes).is_err());
    }
}
