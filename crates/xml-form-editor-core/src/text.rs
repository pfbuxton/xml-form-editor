//! Escaping values for XML, and converting byte offsets to the UTF-16 offsets VS Code uses.

/// Escapes a value for use as element content.
pub(crate) fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            // A literal carriage return would be normalised away by the next parser
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escapes a value for use inside an attribute delimited by `quote`.
pub(crate) fn escape_attribute(value: &str, quote: u8) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' if quote == b'"' => out.push_str("&quot;"),
            '\'' if quote == b'\'' => out.push_str("&apos;"),
            // Parsers turn literal whitespace characters in attributes into spaces
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

/// Converts byte offsets into a string to UTF-16 offsets, which is how VS Code counts positions.
pub(crate) struct Utf16Offsets {
    /// For each non-ASCII character: the byte offset just after it, and how many more bytes than
    /// UTF-16 units the string has up to there
    steps: Vec<(usize, usize)>,
}

impl Utf16Offsets {
    pub(crate) fn new(text: &str) -> Self {
        let mut steps = Vec::new();
        let mut surplus = 0;
        for (i, c) in text.char_indices() {
            if !c.is_ascii() {
                surplus += c.len_utf8() - c.len_utf16();
                steps.push((i + c.len_utf8(), surplus));
            }
        }
        Utf16Offsets { steps }
    }

    /// The UTF-16 offset of `byte`, which must be on a character boundary.
    pub(crate) fn of(&self, byte: usize) -> u32 {
        let k = self.steps.partition_point(|&(end, _)| end <= byte);
        let surplus = if k == 0 { 0 } else { self.steps[k - 1].1 };
        (byte - surplus) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_count_units_not_bytes() {
        // 'µ' is 2 bytes and 1 UTF-16 unit; '𝜓' is 4 bytes and 2 units
        let text = "aµb𝜓c";
        let offsets = Utf16Offsets::new(text);
        let expected: Vec<u32> = [0, 1, 3, 4, 8, 9]
            .iter()
            .map(|&b| text[..b].encode_utf16().count() as u32)
            .collect();
        let actual: Vec<u32> = [0, 1, 3, 4, 8, 9].iter().map(|&b| offsets.of(b)).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn escaping() {
        assert_eq!(
            escape_text("a < b && c > d"),
            "a &lt; b &amp;&amp; c &gt; d"
        );
        assert_eq!(
            escape_attribute(r#"say "hi" & 'bye'"#, b'"'),
            "say &quot;hi&quot; &amp; 'bye'"
        );
        assert_eq!(
            escape_attribute(r#"say "hi" & 'bye'"#, b'\''),
            "say \"hi\" &amp; &apos;bye&apos;"
        );
    }
}
