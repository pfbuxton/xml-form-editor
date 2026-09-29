//! Simple types: XSD's built-in types, the facets that restrict them, and checking a value.

use std::sync::OnceLock;

use regex_lite::Regex;

/// The XSD built-in types a value can have. Types the form treats alike share a variant, e.g.
/// `xs:NCName` is a [`Builtin::Token`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Builtin {
    String,
    NormalizedString,
    Token,
    AnyUri,
    QName,
    Boolean,
    Decimal,
    Integer,
    NonNegativeInteger,
    PositiveInteger,
    NonPositiveInteger,
    NegativeInteger,
    Long,
    Int,
    Short,
    Byte,
    UnsignedLong,
    UnsignedInt,
    UnsignedShort,
    UnsignedByte,
    Float,
    Double,
    Date,
    DateTime,
    Time,
    Duration,
    GYear,
    GYearMonth,
    GMonth,
    GMonthDay,
    GDay,
    HexBinary,
    Base64Binary,
    #[default]
    AnySimpleType,
}

enum Whitespace {
    Preserve,
    Replace,
    Collapse,
}

impl Builtin {
    /// The built-in type with this local name in the XSD namespace.
    pub(crate) fn from_xs_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "string" => Builtin::String,
            "normalizedString" => Builtin::NormalizedString,
            "token" | "language" | "Name" | "NCName" | "ID" | "IDREF" | "IDREFS" | "ENTITY"
            | "ENTITIES" | "NMTOKEN" | "NMTOKENS" | "NOTATION" => Builtin::Token,
            "anyURI" => Builtin::AnyUri,
            "QName" => Builtin::QName,
            "boolean" => Builtin::Boolean,
            "decimal" => Builtin::Decimal,
            "integer" => Builtin::Integer,
            "nonNegativeInteger" => Builtin::NonNegativeInteger,
            "positiveInteger" => Builtin::PositiveInteger,
            "nonPositiveInteger" => Builtin::NonPositiveInteger,
            "negativeInteger" => Builtin::NegativeInteger,
            "long" => Builtin::Long,
            "int" => Builtin::Int,
            "short" => Builtin::Short,
            "byte" => Builtin::Byte,
            "unsignedLong" => Builtin::UnsignedLong,
            "unsignedInt" => Builtin::UnsignedInt,
            "unsignedShort" => Builtin::UnsignedShort,
            "unsignedByte" => Builtin::UnsignedByte,
            "float" => Builtin::Float,
            "double" => Builtin::Double,
            "date" => Builtin::Date,
            "dateTime" | "dateTimeStamp" => Builtin::DateTime,
            "time" => Builtin::Time,
            "duration" | "dayTimeDuration" | "yearMonthDuration" => Builtin::Duration,
            "gYear" => Builtin::GYear,
            "gYearMonth" => Builtin::GYearMonth,
            "gMonth" => Builtin::GMonth,
            "gMonthDay" => Builtin::GMonthDay,
            "gDay" => Builtin::GDay,
            "hexBinary" => Builtin::HexBinary,
            "base64Binary" => Builtin::Base64Binary,
            "anySimpleType" | "anyAtomicType" => Builtin::AnySimpleType,
            _ => return None,
        })
    }

    pub fn xs_name(self) -> &'static str {
        match self {
            Builtin::String => "string",
            Builtin::NormalizedString => "normalizedString",
            Builtin::Token => "token",
            Builtin::AnyUri => "anyURI",
            Builtin::QName => "QName",
            Builtin::Boolean => "boolean",
            Builtin::Decimal => "decimal",
            Builtin::Integer => "integer",
            Builtin::NonNegativeInteger => "nonNegativeInteger",
            Builtin::PositiveInteger => "positiveInteger",
            Builtin::NonPositiveInteger => "nonPositiveInteger",
            Builtin::NegativeInteger => "negativeInteger",
            Builtin::Long => "long",
            Builtin::Int => "int",
            Builtin::Short => "short",
            Builtin::Byte => "byte",
            Builtin::UnsignedLong => "unsignedLong",
            Builtin::UnsignedInt => "unsignedInt",
            Builtin::UnsignedShort => "unsignedShort",
            Builtin::UnsignedByte => "unsignedByte",
            Builtin::Float => "float",
            Builtin::Double => "double",
            Builtin::Date => "date",
            Builtin::DateTime => "dateTime",
            Builtin::Time => "time",
            Builtin::Duration => "duration",
            Builtin::GYear => "gYear",
            Builtin::GYearMonth => "gYearMonth",
            Builtin::GMonth => "gMonth",
            Builtin::GMonthDay => "gMonthDay",
            Builtin::GDay => "gDay",
            Builtin::HexBinary => "hexBinary",
            Builtin::Base64Binary => "base64Binary",
            Builtin::AnySimpleType => "anySimpleType",
        }
    }

    /// The range of an integer type, or `None` for types that aren't integers.
    fn integer_range(self) -> Option<(Option<i128>, Option<i128>)> {
        Some(match self {
            Builtin::Integer => (None, None),
            Builtin::NonNegativeInteger => (Some(0), None),
            Builtin::PositiveInteger => (Some(1), None),
            Builtin::NonPositiveInteger => (None, Some(0)),
            Builtin::NegativeInteger => (None, Some(-1)),
            Builtin::Long => (Some(i64::MIN.into()), Some(i64::MAX.into())),
            Builtin::Int => (Some(i32::MIN.into()), Some(i32::MAX.into())),
            Builtin::Short => (Some(i16::MIN.into()), Some(i16::MAX.into())),
            Builtin::Byte => (Some(i8::MIN.into()), Some(i8::MAX.into())),
            Builtin::UnsignedLong => (Some(0), Some(u64::MAX.into())),
            Builtin::UnsignedInt => (Some(0), Some(u32::MAX.into())),
            Builtin::UnsignedShort => (Some(0), Some(u16::MAX.into())),
            Builtin::UnsignedByte => (Some(0), Some(u8::MAX.into())),
            _ => return None,
        })
    }

    pub fn is_numeric(self) -> bool {
        self.integer_range().is_some()
            || matches!(self, Builtin::Decimal | Builtin::Float | Builtin::Double)
    }

    /// Whether the length facets count this type's characters.
    fn is_textual(self) -> bool {
        matches!(
            self,
            Builtin::String
                | Builtin::NormalizedString
                | Builtin::Token
                | Builtin::AnyUri
                | Builtin::QName
                | Builtin::AnySimpleType
        )
    }

    fn whitespace(self) -> Whitespace {
        match self {
            Builtin::String | Builtin::AnySimpleType => Whitespace::Preserve,
            Builtin::NormalizedString => Whitespace::Replace,
            _ => Whitespace::Collapse,
        }
    }

    /// What a valid value looks like, to finish "Expected …".
    fn expectation(self) -> &'static str {
        match self {
            Builtin::Boolean => "true or false",
            Builtin::Integer | Builtin::Long | Builtin::Int | Builtin::Short | Builtin::Byte => {
                "a whole number"
            }
            Builtin::NonNegativeInteger
            | Builtin::UnsignedLong
            | Builtin::UnsignedInt
            | Builtin::UnsignedShort
            | Builtin::UnsignedByte => "a whole number, 0 or more",
            Builtin::PositiveInteger => "a whole number, 1 or more",
            Builtin::NonPositiveInteger => "a whole number, 0 or less",
            Builtin::NegativeInteger => "a whole number, -1 or less",
            Builtin::Decimal => "a decimal number, such as 1.5",
            Builtin::Float | Builtin::Double => "a number, such as 1.5 or 2.0e-3",
            Builtin::Date => "a date, such as 2024-12-31",
            Builtin::DateTime => "a date and time, such as 2024-12-31T23:59:00",
            Builtin::Time => "a time, such as 23:59:00",
            Builtin::Duration => "a duration, such as P1DT2H or PT30S",
            Builtin::GYear => "a year, such as 2024",
            Builtin::GYearMonth => "a year and month, such as 2024-12",
            Builtin::GMonth => "a month, such as --12",
            Builtin::GMonthDay => "a month and day, such as --12-31",
            Builtin::GDay => "a day, such as ---31",
            Builtin::HexBinary => "pairs of hexadecimal digits",
            Builtin::Base64Binary => "base64 text",
            _ => "text",
        }
    }

    /// Checks the form of a value that has already had its whitespace handled.
    fn check_lexical(self, value: &str) -> Result<(), String> {
        if let Some((min, max)) = self.integer_range() {
            return check_integer(self, value, min, max);
        }
        let valid = match self {
            Builtin::Boolean => matches!(value, "true" | "false" | "1" | "0"),
            Builtin::Decimal => is_decimal(value),
            Builtin::Float | Builtin::Double => is_double(value),
            Builtin::Duration => {
                lexical_regex(self).is_some_and(|re| re.is_match(value))
                    && value.bytes().any(|b| b.is_ascii_digit())
                    && !value.ends_with('T')
            }
            Builtin::HexBinary => {
                value.len().is_multiple_of(2) && value.bytes().all(|b| b.is_ascii_hexdigit())
            }
            Builtin::Base64Binary => value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b' ')),
            _ => lexical_regex(self).is_none_or(|re| re.is_match(value)),
        };
        if valid {
            Ok(())
        } else {
            Err(self.lexical_error(value))
        }
    }

    fn lexical_error(self, value: &str) -> String {
        if value.is_empty() {
            format!("Empty: expected {}", self.expectation())
        } else {
            format!("Expected {}", self.expectation())
        }
    }
}

fn check_integer(
    ty: Builtin,
    value: &str,
    min: Option<i128>,
    max: Option<i128>,
) -> Result<(), String> {
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ty.lexical_error(value));
    }
    let out_of_range = match value.parse::<i128>() {
        Ok(n) => min.is_some_and(|min| n < min) || max.is_some_and(|max| n > max),
        // Too many digits for an i128: only fits a type with no bound on that side
        Err(_) => {
            let negative = value.starts_with('-');
            (negative && min.is_some()) || (!negative && max.is_some())
        }
    };
    if !out_of_range {
        return Ok(());
    }
    Err(match (min, max) {
        (Some(min), Some(max)) => format!("Must be a whole number from {min} to {max}"),
        (Some(min), None) => format!("Must be a whole number, {min} or more"),
        (None, Some(max)) => format!("Must be a whole number, {max} or less"),
        (None, None) => unreachable!("an unbounded integer is never out of range"),
    })
}

fn is_decimal(value: &str) -> bool {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    (!whole.is_empty() || !fraction.is_empty())
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
}

fn is_double(value: &str) -> bool {
    if matches!(value, "INF" | "+INF" | "-INF" | "NaN") {
        return true;
    }
    let (mantissa, exponent) = match value.find(['e', 'E']) {
        Some(i) => (&value[..i], Some(&value[i + 1..])),
        None => (value, None),
    };
    is_decimal(mantissa)
        && exponent.is_none_or(|e| {
            let digits = e.strip_prefix(['+', '-']).unwrap_or(e);
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        })
}

/// Parses a number as XSD writes them, including `INF`, `-INF` and `NaN`.
fn parse_number(value: &str) -> Option<f64> {
    match value {
        "INF" | "+INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => value.parse().ok(),
    }
}

/// The lexical form of the date and time types.
fn lexical_regex(ty: Builtin) -> Option<&'static Regex> {
    static CACHE: [OnceLock<Regex>; 9] = [const { OnceLock::new() }; 9];
    let (slot, pattern) = match ty {
        Builtin::Date => (
            0,
            r"^-?\d{4,}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])(Z|[+-]\d{2}:\d{2})?$",
        ),
        Builtin::DateTime => (
            1,
            r"^-?\d{4,}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])T([01]\d|2[0-4]):[0-5]\d:[0-5]\d(\.\d+)?(Z|[+-]\d{2}:\d{2})?$",
        ),
        Builtin::Time => (
            2,
            r"^([01]\d|2[0-4]):[0-5]\d:[0-5]\d(\.\d+)?(Z|[+-]\d{2}:\d{2})?$",
        ),
        Builtin::Duration => (
            3,
            r"^-?P(\d+Y)?(\d+M)?(\d+D)?(T(\d+H)?(\d+M)?(\d+(\.\d+)?S)?)?$",
        ),
        Builtin::GYear => (4, r"^-?\d{4,}(Z|[+-]\d{2}:\d{2})?$"),
        Builtin::GYearMonth => (5, r"^-?\d{4,}-(0[1-9]|1[0-2])(Z|[+-]\d{2}:\d{2})?$"),
        Builtin::GMonth => (6, r"^--(0[1-9]|1[0-2])(Z|[+-]\d{2}:\d{2})?$"),
        Builtin::GMonthDay => (
            7,
            r"^--(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])(Z|[+-]\d{2}:\d{2})?$",
        ),
        Builtin::GDay => (8, r"^---(0[1-9]|[12]\d|3[01])(Z|[+-]\d{2}:\d{2})?$"),
        _ => return None,
    };
    Some(CACHE[slot].get_or_init(|| Regex::new(pattern).expect("the built-in patterns are valid")))
}

/// A lower or upper bound on a value (`xs:minInclusive`, `xs:maxExclusive`, …).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Bound {
    pub value: String,
    pub inclusive: bool,
}

/// One of the values an enumeration allows, with its `xs:documentation`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnumValue {
    pub value: String,
    pub doc: Option<String>,
}

/// The facets of one `xs:restriction`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Facets {
    pub enumeration: Vec<EnumValue>,
    pub patterns: Vec<String>,
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub length: Option<u64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
}

impl Facets {
    pub(crate) fn is_empty(&self) -> bool {
        self.enumeration.is_empty()
            && self.patterns.is_empty()
            && self.min.is_none()
            && self.max.is_none()
            && self.length.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
    }
}

/// A simple type with its derivation resolved: everything needed to show and check a value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SimpleInfo {
    /// The type's name in the schema; `None` for built-in and anonymous types
    pub name: Option<String>,
    /// The built-in type it derives from
    pub builtin: Builtin,
    /// The allowed values; empty when any value of the type is allowed
    pub enumeration: Vec<EnumValue>,
    /// Pattern facets: a value must match one pattern of each derivation step
    pub patterns: Vec<Vec<String>>,
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub length: Option<u64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    /// For a list type, the type of its items
    pub item: Option<Box<SimpleInfo>>,
    /// For a union type, its member types
    pub members: Vec<SimpleInfo>,
    /// The type's `xs:documentation`
    pub doc: Option<String>,
}

impl SimpleInfo {
    pub(crate) fn builtin(builtin: Builtin) -> SimpleInfo {
        SimpleInfo {
            builtin,
            ..SimpleInfo::default()
        }
    }

    pub(crate) fn list(item: SimpleInfo) -> SimpleInfo {
        SimpleInfo {
            item: Some(Box::new(item)),
            ..SimpleInfo::default()
        }
    }

    pub(crate) fn union(members: Vec<SimpleInfo>) -> SimpleInfo {
        SimpleInfo {
            members,
            ..SimpleInfo::default()
        }
    }

    /// Narrows the type by the facets of a restriction.
    pub(crate) fn restrict(&mut self, facets: &Facets) {
        if !facets.enumeration.is_empty() {
            self.enumeration = facets.enumeration.clone();
        }
        if !facets.patterns.is_empty() {
            self.patterns.push(facets.patterns.clone());
        }
        if facets.min.is_some() {
            self.min = facets.min.clone();
        }
        if facets.max.is_some() {
            self.max = facets.max.clone();
        }
        if facets.length.is_some() {
            self.length = facets.length;
        }
        if facets.min_length.is_some() {
            self.min_length = facets.min_length;
        }
        if facets.max_length.is_some() {
            self.max_length = facets.max_length;
        }
    }

    pub fn is_list(&self) -> bool {
        self.item.is_some()
    }

    pub fn is_union(&self) -> bool {
        !self.members.is_empty()
    }

    /// Whether any `true` or `false` value is allowed, so the form can show a checkbox.
    pub fn is_boolean(&self) -> bool {
        self.builtin == Builtin::Boolean
            && !self.is_list()
            && !self.is_union()
            && self.enumeration.is_empty()
    }

    fn whitespace(&self) -> Whitespace {
        if self.is_list() || self.is_union() {
            Whitespace::Collapse
        } else {
            self.builtin.whitespace()
        }
    }

    /// The value with its whitespace handled as the type specifies: for most types other than
    /// strings, leading and trailing whitespace is dropped and runs of whitespace become one space.
    pub fn normalize(&self, value: &str) -> String {
        match self.whitespace() {
            Whitespace::Preserve => value.to_string(),
            Whitespace::Replace => value.replace(['\t', '\n', '\r'], " "),
            Whitespace::Collapse => value.split_whitespace().collect::<Vec<_>>().join(" "),
        }
    }

    /// Checks a value against the type, returning why it isn't valid.
    pub fn validate(&self, value: &str) -> Result<(), String> {
        let value = self.normalize(value);
        if let Some(item) = &self.item {
            let items: Vec<&str> = value.split_whitespace().collect();
            self.check_length(items.len() as u64, "items")?;
            for (i, it) in items.iter().enumerate() {
                item.validate(it)
                    .map_err(|e| format!("Item {} ({it}): {e}", i + 1))?;
            }
        } else if self.is_union() {
            if !self.members.iter().any(|m| m.validate(&value).is_ok()) {
                let names: Vec<String> = self.members.iter().map(SimpleInfo::summary).collect();
                return Err(format!(
                    "Expected a value of one of the types: {}",
                    names.join(", ")
                ));
            }
        } else if self.enumeration.is_empty() {
            self.builtin.check_lexical(&value)?;
            if self.builtin.is_textual() {
                self.check_length(value.chars().count() as u64, "characters")?;
            }
            if self.builtin.is_numeric() {
                self.check_bounds(&value)?;
            }
        }
        if !self.enumeration.is_empty()
            && !self
                .enumeration
                .iter()
                .any(|e| self.normalize(&e.value) == value)
        {
            let allowed: Vec<&str> = self.enumeration.iter().map(|e| e.value.as_str()).collect();
            return Err(format!("Must be one of: {}", allowed.join(", ")));
        }
        self.check_patterns(&value)
    }

    fn check_length(&self, length: u64, unit: &str) -> Result<(), String> {
        if let Some(exact) = self.length.filter(|&n| n != length) {
            return Err(format!("Must have exactly {exact} {unit}"));
        }
        if let Some(min) = self.min_length.filter(|&n| length < n) {
            return Err(format!("Must have at least {min} {unit}"));
        }
        if let Some(max) = self.max_length.filter(|&n| length > n) {
            return Err(format!("Must have at most {max} {unit}"));
        }
        Ok(())
    }

    fn check_bounds(&self, value: &str) -> Result<(), String> {
        let Some(number) = parse_number(value) else {
            return Ok(());
        };
        if let Some(min) = &self.min
            && let Some(limit) = parse_number(&min.value)
        {
            let ok = if min.inclusive {
                number >= limit
            } else {
                number > limit
            };
            if !ok {
                let relation = if min.inclusive { "≥" } else { ">" };
                return Err(format!("Must be {relation} {}", min.value));
            }
        }
        if let Some(max) = &self.max
            && let Some(limit) = parse_number(&max.value)
        {
            let ok = if max.inclusive {
                number <= limit
            } else {
                number < limit
            };
            if !ok {
                let relation = if max.inclusive { "≤" } else { "<" };
                return Err(format!("Must be {relation} {}", max.value));
            }
        }
        Ok(())
    }

    fn check_patterns(&self, value: &str) -> Result<(), String> {
        for step in &self.patterns {
            // Patterns in XSD's dialect that this regex engine can't compile are skipped
            let compiled: Vec<Regex> = step
                .iter()
                .filter_map(|p| Regex::new(&format!("^(?:{p})$")).ok())
                .collect();
            if !compiled.is_empty() && !compiled.iter().any(|re| re.is_match(value)) {
                return Err(match step.as_slice() {
                    [pattern] => format!("Must match the pattern {pattern}"),
                    _ => format!("Must match one of the patterns {}", step.join(", ")),
                });
            }
        }
        Ok(())
    }

    /// A short description of the type, such as `nonNegativeInteger`, `PositiveDouble: double > 0`
    /// or `DoubleList: list of double`.
    pub fn summary(&self) -> String {
        let base = if let Some(item) = &self.item {
            format!("list of {}", item.summary())
        } else if self.is_union() {
            self.members
                .iter()
                .map(SimpleInfo::summary)
                .collect::<Vec<_>>()
                .join(" | ")
        } else {
            let mut base = self.builtin.xs_name().to_string();
            if let Some(min) = &self.min {
                base.push_str(&format!(
                    " {} {}",
                    if min.inclusive { "≥" } else { ">" },
                    min.value
                ));
            }
            if let Some(max) = &self.max {
                base.push_str(&format!(
                    " {} {}",
                    if max.inclusive { "≤" } else { "<" },
                    max.value
                ));
            }
            base
        };
        match &self.name {
            Some(name) if !self.enumeration.is_empty() => name.clone(),
            Some(name) => format!("{name}: {base}"),
            None if !self.enumeration.is_empty() => {
                format!("{} (one of a list)", self.builtin.xs_name())
            }
            None => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(b: Builtin) -> SimpleInfo {
        SimpleInfo::builtin(b)
    }

    #[test]
    fn numbers() {
        let double = builtin(Builtin::Double);
        for ok in [
            "1", "-1.5", "5.0e-6", "10.0E+3", ".5", "5.", "INF", "-INF", "NaN", "  2.5 ",
        ] {
            assert!(double.validate(ok).is_ok(), "{ok} is a double");
        }
        for bad in ["", "abc", "1e", "1.2.3", "inf", "1,5", "e5"] {
            assert!(double.validate(bad).is_err(), "{bad} is not a double");
        }
        let count = builtin(Builtin::NonNegativeInteger);
        assert!(count.validate("0").is_ok());
        assert!(count.validate("+10").is_ok());
        assert!(
            count
                .validate("123456789012345678901234567890123456789012345")
                .is_ok()
        );
        assert_eq!(
            count.validate("-1").unwrap_err(),
            "Must be a whole number, 0 or more"
        );
        assert_eq!(
            count.validate("1.5").unwrap_err(),
            "Expected a whole number, 0 or more"
        );
        assert_eq!(
            builtin(Builtin::Int).validate("3000000000").unwrap_err(),
            "Must be a whole number from -2147483648 to 2147483647"
        );
    }

    #[test]
    fn facets() {
        let mut positive = builtin(Builtin::Double);
        positive.name = Some("PositiveDouble".into());
        positive.restrict(&Facets {
            min: Some(Bound {
                value: "0.0".into(),
                inclusive: false,
            }),
            ..Facets::default()
        });
        assert_eq!(positive.validate("0").unwrap_err(), "Must be > 0.0");
        assert!(positive.validate("1.0").is_ok());
        assert_eq!(positive.summary(), "PositiveDouble: double > 0.0");

        let mut method = builtin(Builtin::String);
        method.name = Some("Method".into());
        method.restrict(&Facets {
            enumeration: vec![
                EnumValue {
                    value: "picard".into(),
                    doc: None,
                },
                EnumValue {
                    value: "newton_krylov".into(),
                    doc: None,
                },
            ],
            ..Facets::default()
        });
        assert!(method.validate("picard").is_ok());
        assert_eq!(
            method.validate("newton").unwrap_err(),
            "Must be one of: picard, newton_krylov"
        );

        let mut code = builtin(Builtin::Token);
        code.restrict(&Facets {
            patterns: vec!["[A-Z]{3}\\d+".into()],
            ..Facets::default()
        });
        assert!(code.validate("RUN16").is_ok());
        assert!(code.validate("run16").is_err());
    }

    #[test]
    fn lists_and_booleans() {
        let list = SimpleInfo::list(builtin(Builtin::Double));
        assert!(list.validate("0.1 0.2  0.3").is_ok());
        assert!(list.validate("").is_ok());
        assert_eq!(
            list.validate("0.1 x").unwrap_err(),
            "Item 2 (x): Expected a number, such as 1.5 or 2.0e-3"
        );
        assert_eq!(list.summary(), "list of double");

        let boolean = builtin(Builtin::Boolean);
        assert!(boolean.is_boolean());
        assert!(boolean.validate(" true ").is_ok());
        assert!(boolean.validate("yes").is_err());
    }

    #[test]
    fn dates() {
        assert!(builtin(Builtin::Date).validate("2024-12-31").is_ok());
        assert!(builtin(Builtin::Date).validate("2024-13-01").is_err());
        assert!(
            builtin(Builtin::DateTime)
                .validate("2024-12-31T23:59:00Z")
                .is_ok()
        );
        assert!(builtin(Builtin::Duration).validate("PT30S").is_ok());
        assert!(builtin(Builtin::Duration).validate("P").is_err());
        assert!(builtin(Builtin::Duration).validate("P1DT").is_err());
    }
}
