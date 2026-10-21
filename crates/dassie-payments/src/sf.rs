//! The part of Structured Field Values (RFC 8941) that HTTP message
//! signatures and `Content-Digest` use: dictionaries of items and inner
//! lists, with parameters. Decimals are refused, since no field here uses
//! them.
//!
//! Everything is bounded: the field length, the members of a dictionary,
//! the items of an inner list and the parameters of each
//! (spec/decisions/tigerstyle.md).

use alloc::string::String;
use alloc::vec::Vec;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// The longest field value parsed, in bytes.
pub const MAX_FIELD_BYTES: usize = 8 * 1024;
/// The most members a dictionary may have.
pub const MAX_MEMBERS: usize = 16;
/// The most items an inner list may have.
pub const MAX_INNER_ITEMS: usize = 32;
/// The most parameters an item or inner list may have.
pub const MAX_PARAMETERS: usize = 16;
const MAX_INTEGER: i64 = 999_999_999_999_999;

/// The field is not a structured field this parser accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidField;

/// A bare item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BareItem {
    /// An integer of at most 15 digits.
    Integer(i64),
    /// A string of printable ASCII.
    String(String),
    /// A token.
    Token(String),
    /// A byte sequence.
    Bytes(Vec<u8>),
    /// A boolean.
    Boolean(bool),
}

/// Parameters in order, with each key at most once.
pub type Parameters = Vec<(String, BareItem)>;

/// An item with its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The value.
    pub bare: BareItem,
    /// Its parameters.
    pub parameters: Parameters,
}

/// A dictionary member's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    /// A single item.
    Item(Item),
    /// An inner list and its parameters.
    InnerList(Vec<Item>, Parameters),
}

/// A dictionary, in order, with each key at most once.
pub type Dictionary = Vec<(String, Member)>;

/// Looks a parameter up by key.
#[must_use]
pub fn parameter<'a>(parameters: &'a Parameters, key: &str) -> Option<&'a BareItem> {
    parameters
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// Parses a dictionary field (RFC 8941 §4.2.2). Later duplicates replace
/// earlier ones in place.
///
/// # Errors
///
/// If the field is malformed, uses a decimal, or exceeds a bound.
pub fn parse_dictionary(input: &str) -> Result<Dictionary, InvalidField> {
    if input.len() > MAX_FIELD_BYTES {
        return Err(InvalidField);
    }
    let mut parser = Parser(input.trim_matches(' ').as_bytes());
    let mut dictionary = Dictionary::new();
    while !parser.0.is_empty() {
        let key = parser.key()?;
        let member = if parser.eat(b'=') {
            parser.member()?
        } else {
            Member::Item(Item {
                bare: BareItem::Boolean(true),
                parameters: parser.parameters()?,
            })
        };
        insert(&mut dictionary, key, member, MAX_MEMBERS)?;
        parser.whitespace();
        if parser.0.is_empty() {
            break;
        }
        if !parser.eat(b',') {
            return Err(InvalidField);
        }
        parser.whitespace();
        if parser.0.is_empty() {
            return Err(InvalidField);
        }
    }
    Ok(dictionary)
}

fn insert<T>(
    entries: &mut Vec<(String, T)>,
    key: String,
    value: T,
    limit: usize,
) -> Result<(), InvalidField> {
    if let Some(entry) = entries.iter_mut().find(|(name, _)| *name == key) {
        entry.1 = value;
        return Ok(());
    }
    if entries.len() >= limit {
        return Err(InvalidField);
    }
    entries.push((key, value));
    Ok(())
}

struct Parser<'a>(&'a [u8]);

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(first)
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.bump();
        }
    }

    fn spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.bump();
        }
    }

    fn key(&mut self) -> Result<String, InvalidField> {
        if !matches!(self.peek(), Some(b'a'..=b'z' | b'*')) {
            return Err(InvalidField);
        }
        let mut key = String::new();
        while let Some(byte @ (b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'*')) = self.peek()
        {
            self.bump();
            key.push(char::from(byte));
        }
        Ok(key)
    }

    fn member(&mut self) -> Result<Member, InvalidField> {
        if self.peek() == Some(b'(') {
            self.inner_list()
        } else {
            Ok(Member::Item(self.item()?))
        }
    }

    fn inner_list(&mut self) -> Result<Member, InvalidField> {
        self.bump();
        let mut items = Vec::new();
        loop {
            self.spaces();
            if self.eat(b')') {
                return Ok(Member::InnerList(items, self.parameters()?));
            }
            if items.len() >= MAX_INNER_ITEMS {
                return Err(InvalidField);
            }
            items.push(self.item()?);
            if !matches!(self.peek(), Some(b' ' | b')')) {
                return Err(InvalidField);
            }
        }
    }

    fn item(&mut self) -> Result<Item, InvalidField> {
        let bare = self.bare_item()?;
        Ok(Item {
            bare,
            parameters: self.parameters()?,
        })
    }

    fn parameters(&mut self) -> Result<Parameters, InvalidField> {
        let mut parameters = Parameters::new();
        while self.eat(b';') {
            self.spaces();
            let key = self.key()?;
            let value = if self.eat(b'=') {
                self.bare_item()?
            } else {
                BareItem::Boolean(true)
            };
            insert(&mut parameters, key, value, MAX_PARAMETERS)?;
        }
        Ok(parameters)
    }

    fn bare_item(&mut self) -> Result<BareItem, InvalidField> {
        match self.peek() {
            Some(b'-' | b'0'..=b'9') => self.integer(),
            Some(b'"') => self.string(),
            Some(b':') => self.bytes(),
            Some(b'?') => self.boolean(),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'*') => Ok(self.token()),
            _ => Err(InvalidField),
        }
    }

    fn integer(&mut self) -> Result<BareItem, InvalidField> {
        let negative = self.eat(b'-');
        let mut value = 0i64;
        let mut digits = 0usize;
        while let Some(digit @ b'0'..=b'9') = self.peek() {
            self.bump();
            digits += 1;
            if digits > 15 {
                return Err(InvalidField);
            }
            value = value * 10 + i64::from(digit - b'0');
        }
        // A decimal, or a sign alone.
        if digits == 0 || self.peek() == Some(b'.') {
            return Err(InvalidField);
        }
        assert!(value <= MAX_INTEGER, "fifteen digits fit");
        Ok(BareItem::Integer(if negative { -value } else { value }))
    }

    fn string(&mut self) -> Result<BareItem, InvalidField> {
        self.bump();
        let mut value = String::new();
        loop {
            match self.bump() {
                Some(b'"') => return Ok(BareItem::String(value)),
                Some(b'\\') => match self.bump() {
                    Some(escaped @ (b'"' | b'\\')) => value.push(char::from(escaped)),
                    _ => return Err(InvalidField),
                },
                Some(byte @ 0x20..=0x7e) => value.push(char::from(byte)),
                _ => return Err(InvalidField),
            }
        }
    }

    fn token(&mut self) -> BareItem {
        let mut value = String::new();
        while let Some(byte) = self.peek() {
            let tchar = byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&byte);
            if !tchar {
                break;
            }
            self.bump();
            value.push(char::from(byte));
        }
        BareItem::Token(value)
    }

    fn bytes(&mut self) -> Result<BareItem, InvalidField> {
        self.bump();
        let end = self
            .0
            .iter()
            .position(|&byte| byte == b':')
            .ok_or(InvalidField)?;
        let (encoded, rest) = self.0.split_at(end);
        let valid = encoded
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='));
        if !valid {
            return Err(InvalidField);
        }
        let decoded = STANDARD.decode(encoded).map_err(|_| InvalidField)?;
        self.0 = rest.get(1..).unwrap_or_default();
        Ok(BareItem::Bytes(decoded))
    }

    fn boolean(&mut self) -> Result<BareItem, InvalidField> {
        self.bump();
        match self.bump() {
            Some(b'1') => Ok(BareItem::Boolean(true)),
            Some(b'0') => Ok(BareItem::Boolean(false)),
            _ => Err(InvalidField),
        }
    }
}

/// Serializes an inner list with its parameters (RFC 8941 §4.1.1.1).
///
/// # Panics
///
/// If a string holds a character a structured field cannot carry, or a
/// token or key is empty: the caller builds these, so that is a bug.
#[must_use]
pub fn serialize_inner_list(items: &[Item], parameters: &Parameters) -> String {
    let mut output = String::from("(");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        serialize_bare(&item.bare, &mut output);
        serialize_parameters(&item.parameters, &mut output);
    }
    output.push(')');
    serialize_parameters(parameters, &mut output);
    output
}

/// Serializes a dictionary whose members are byte sequences, as the
/// `Signature` and `Content-Digest` fields are.
#[must_use]
pub fn serialize_byte_dictionary(members: &[(&str, &[u8])]) -> String {
    let mut output = String::new();
    for (index, (key, bytes)) in members.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        assert!(!key.is_empty(), "empty key");
        output.push_str(key);
        output.push('=');
        serialize_bare(&BareItem::Bytes(bytes.to_vec()), &mut output);
    }
    output
}

fn serialize_parameters(parameters: &Parameters, output: &mut String) {
    for (key, value) in parameters {
        assert!(!key.is_empty(), "empty parameter key");
        output.push(';');
        output.push_str(key);
        if *value != BareItem::Boolean(true) {
            output.push('=');
            serialize_bare(value, output);
        }
    }
}

fn serialize_bare(bare: &BareItem, output: &mut String) {
    match bare {
        BareItem::Integer(value) => {
            assert!(
                value.unsigned_abs() <= MAX_INTEGER.unsigned_abs(),
                "integer too long"
            );
            output.push_str(&alloc::format!("{value}"));
        }
        BareItem::String(value) => {
            output.push('"');
            for character in value.chars() {
                assert!(
                    matches!(character, ' '..='~'),
                    "unprintable string character"
                );
                if matches!(character, '"' | '\\') {
                    output.push('\\');
                }
                output.push(character);
            }
            output.push('"');
        }
        BareItem::Token(value) => {
            assert!(!value.is_empty(), "empty token");
            output.push_str(value);
        }
        BareItem::Bytes(value) => {
            output.push(':');
            output.push_str(&STANDARD.encode(value));
            output.push(':');
        }
        BareItem::Boolean(value) => output.push_str(if *value { "?1" } else { "?0" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn string(value: &str) -> Item {
        Item {
            bare: BareItem::String(value.into()),
            parameters: Parameters::new(),
        }
    }

    #[test]
    fn parses_a_signature_input() {
        let field = r#"sig1=("@method" "@target-uri");created=1618884473;keyid="test-key-ed25519""#;
        let dictionary = parse_dictionary(field).unwrap();
        let [(label, Member::InnerList(items, parameters))] = dictionary.as_slice() else {
            panic!("not one inner list: {dictionary:?}");
        };
        assert_eq!(label, "sig1");
        assert_eq!(items, &vec![string("@method"), string("@target-uri")]);
        assert_eq!(
            parameter(parameters, "created"),
            Some(&BareItem::Integer(1_618_884_473))
        );
        assert_eq!(
            serialize_inner_list(items, parameters),
            r#"("@method" "@target-uri");created=1618884473;keyid="test-key-ed25519""#
        );
    }

    #[test]
    fn follows_the_rfc_examples() {
        // RFC 8941 §3.2.
        let dictionary = parse_dictionary("en=\"Applepie\", da=:w4ZibGV0w6ZydGU=:").unwrap();
        assert_eq!(
            dictionary,
            vec![
                ("en".into(), Member::Item(string("Applepie"))),
                (
                    "da".into(),
                    Member::Item(Item {
                        bare: BareItem::Bytes("Æbletærte".as_bytes().to_vec()),
                        parameters: Parameters::new(),
                    })
                ),
            ]
        );
        let flags = parse_dictionary("a=?0, b, c; foo=bar").unwrap();
        assert_eq!(flags.len(), 3);
        assert_eq!(
            flags.get(2).map(|(_, member)| member),
            Some(&Member::Item(Item {
                bare: BareItem::Boolean(true),
                parameters: vec![("foo".into(), BareItem::Token("bar".into()))],
            }))
        );
    }

    #[test]
    fn a_later_duplicate_replaces_the_earlier() {
        let dictionary = parse_dictionary("a=1, b=2, a=3").unwrap();
        assert_eq!(
            dictionary.first(),
            Some(&(
                "a".into(),
                Member::Item(Item {
                    bare: BareItem::Integer(3),
                    parameters: Parameters::new()
                })
            ))
        );
    }

    #[test]
    fn refuses_malformed_fields() {
        for field in [
            "a=",
            "a=1,",
            "A=1",
            "a=1.5",
            "a=(1 2",
            "a=\"open",
            "a=:not base64*:",
            "a=?2",
            "a=1;",
            "a=-",
            "a=1234567890123456",
        ] {
            assert_eq!(parse_dictionary(field), Err(InvalidField), "{field}");
        }
    }
}
