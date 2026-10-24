//! The ledger's binary format, any of it, for what the node reads to prove
//! rather than signs (spec/designs/xrpl-validation.md#8-the-verifier):
//! validations, manifests, and later ledger objects, transactions and their
//! metadata.
//!
//! The walker knows only the types, never which fields a kind of record
//! has: that is each reader's to check. It takes only the canonical form,
//! so bytes are read only if they write back to themselves
//! ([`write_field`]): within each object, fields strictly ascending by type
//! code and then field code, so none twice; every header in its shortest
//! form; lengths by type, and a length prefix only in the form its length
//! needs; objects and arrays nested at most [`MAX_DEPTH`] deep, an array
//! holding only objects; and values in the one form rippled writes them in.
//! Untrusted bytes get an error, never a panic, and nothing recurses: an
//! object's end is found with a stack of [`MAX_DEPTH`] frames.
//!
//! The strict codec for what the node signs (`codec`) stays apart.

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::amount::{Amount, TokenAmount, TokenValue};
use crate::codec::DecodeError;
use crate::keys::AccountId;

/// The deepest a field is nested: a field of the serialization itself is at
/// depth 1, and each object or array around it adds one. rippled reads no
/// deeper.
pub const MAX_DEPTH: usize = 10;

/// The longest variable-length value: the most a length prefix writes.
pub const MAX_VL_BYTES: usize = 918_744;

/// The longest length a prefix of one byte writes, and of two.
const MAX_VL_ONE_BYTE: usize = 192;
const MAX_VL_TWO_BYTES: usize = 12_480;

/// The serialized types, by their type codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// Two bytes.
    UInt16,
    /// Four bytes.
    UInt32,
    /// Eight bytes.
    UInt64,
    /// Sixteen bytes.
    Hash128,
    /// 32 bytes.
    Hash256,
    /// XRP in 8 bytes, a token in 48, an MPT in 33.
    Amount,
    /// Bytes with a length prefix.
    Blob,
    /// An account's 20 bytes, or none, with a length prefix.
    AccountId,
    /// A decimal of a 64-bit mantissa and a 32-bit exponent.
    Number,
    /// Four bytes, signed.
    Int32,
    /// Eight bytes, signed.
    Int64,
    /// Fields, then the object's end marker.
    Object,
    /// Objects, then the array's end marker.
    Array,
    /// One byte.
    UInt8,
    /// 20 bytes.
    Hash160,
    /// Payment paths, ended by a zero byte.
    PathSet,
    /// 32-byte hashes, with a length prefix.
    Vector256,
    /// Twelve bytes.
    UInt96,
    /// 24 bytes.
    Hash192,
    /// 48 bytes.
    UInt384,
    /// 64 bytes.
    UInt512,
    /// XRP, a currency and its issuer, or an MPT.
    Issue,
    /// A currency code's 20 bytes.
    Currency,
}

impl Type {
    /// Its type code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::UInt16 => 1,
            Self::UInt32 => 2,
            Self::UInt64 => 3,
            Self::Hash128 => 4,
            Self::Hash256 => 5,
            Self::Amount => 6,
            Self::Blob => 7,
            Self::AccountId => 8,
            Self::Number => 9,
            Self::Int32 => 10,
            Self::Int64 => 11,
            Self::Object => 14,
            Self::Array => 15,
            Self::UInt8 => 16,
            Self::Hash160 => 17,
            Self::PathSet => 18,
            Self::Vector256 => 19,
            Self::UInt96 => 20,
            Self::Hash192 => 21,
            Self::UInt384 => 22,
            Self::UInt512 => 23,
            Self::Issue => 24,
            Self::Currency => 26,
        }
    }

    /// The type of `code`, if the ledger has one.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::UInt16,
            2 => Self::UInt32,
            3 => Self::UInt64,
            4 => Self::Hash128,
            5 => Self::Hash256,
            6 => Self::Amount,
            7 => Self::Blob,
            8 => Self::AccountId,
            9 => Self::Number,
            10 => Self::Int32,
            11 => Self::Int64,
            14 => Self::Object,
            15 => Self::Array,
            16 => Self::UInt8,
            17 => Self::Hash160,
            18 => Self::PathSet,
            19 => Self::Vector256,
            20 => Self::UInt96,
            21 => Self::Hash192,
            22 => Self::UInt384,
            23 => Self::UInt512,
            24 => Self::Issue,
            26 => Self::Currency,
            _ => return None,
        })
    }

    /// Its length, for a type of one length.
    const fn fixed_bytes(self) -> Option<usize> {
        Some(match self {
            Self::UInt8 => 1,
            Self::UInt16 => 2,
            Self::UInt32 | Self::Int32 => 4,
            Self::UInt64 | Self::Int64 => 8,
            Self::UInt96 | Self::Number => 12,
            Self::Hash128 => 16,
            Self::Hash160 | Self::Currency => 20,
            Self::Hash192 => 24,
            Self::Hash256 => 32,
            Self::UInt384 => 48,
            Self::UInt512 => 64,
            _ => return None,
        })
    }

    /// Whether its value has a length prefix.
    const fn has_length_prefix(self) -> bool {
        matches!(self, Self::Blob | Self::AccountId | Self::Vector256)
    }

    /// Whether it holds fields, and ends with a marker.
    const fn is_container(self) -> bool {
        matches!(self, Self::Object | Self::Array)
    }
}

/// A field: its type and its code within the type. Fields sort by type
/// code and then field code, the canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldId {
    kind: Type,
    code: u8,
}

impl FieldId {
    /// The field `code` of type `kind`. Code zero is no field (a bug).
    #[must_use]
    pub const fn new(kind: Type, code: u8) -> Self {
        assert!(code != 0, "field codes start at one");
        Self { kind, code }
    }

    /// Its type.
    #[must_use]
    pub const fn kind(self) -> Type {
        self.kind
    }

    /// Its code within its type.
    #[must_use]
    pub const fn code(self) -> u8 {
        self.code
    }

    const fn sort_key(self) -> u16 {
        (self.kind.code() as u16) << 8 | self.code as u16
    }
}

impl PartialOrd for FieldId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FieldId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

/// What ends an object, and an array.
pub const OBJECT_END: FieldId = FieldId::new(Type::Object, 1);
/// What ends an array.
pub const ARRAY_END: FieldId = FieldId::new(Type::Array, 1);

// An amount's first bits.
const AMOUNT_NOT_XRP: u64 = 0x8000_0000_0000_0000;
const AMOUNT_POSITIVE: u64 = 0x4000_0000_0000_0000;
const AMOUNT_MPT: u64 = 0x2000_0000_0000_0000;

/// The account an MPT's issue names in place of an issuer: one.
const NO_ACCOUNT: [u8; 20] = {
    let mut account = [0; 20];
    account[19] = 1;
    account
};

// A path element's kinds.
const PATH_END: u8 = 0x00;
const PATH_BOUNDARY: u8 = 0xFF;
const PATH_ACCOUNT: u8 = 0x01;
const PATH_CURRENCY: u8 = 0x10;
const PATH_ISSUER: u8 = 0x20;
const PATH_MPT: u8 = 0x40;

/// A field as read: its id and its value.
#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
    /// Which field.
    pub id: FieldId,
    /// Its value as written, without a length prefix or an end marker: an
    /// object's fields, an array's objects, a path set's paths with the
    /// byte that ends them.
    pub value: &'a [u8],
    /// How deep it is (1 for a field of the serialization itself).
    depth: usize,
}

impl<'a> Field<'a> {
    /// Its value, if it is of type `kind`.
    fn of(&self, kind: Type) -> Result<&'a [u8], DecodeError> {
        if self.id.kind == kind {
            Ok(self.value)
        } else {
            Err(DecodeError::Value)
        }
    }

    fn array<const N: usize>(&self, kind: Type) -> Result<[u8; N], DecodeError> {
        self.of(kind)?.try_into().map_err(|_| DecodeError::Value)
    }

    /// A `UInt8`'s value.
    ///
    /// # Errors
    ///
    /// It is of another type ([`DecodeError::Value`]), as is the case for
    /// every reader below.
    pub fn uint8(&self) -> Result<u8, DecodeError> {
        Ok(u8::from_be_bytes(self.array(Type::UInt8)?))
    }

    /// A `UInt16`'s value.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn uint16(&self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array(Type::UInt16)?))
    }

    /// A `UInt32`'s value.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn uint32(&self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array(Type::UInt32)?))
    }

    /// A `UInt64`'s value.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn uint64(&self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array(Type::UInt64)?))
    }

    /// A `Hash128`'s bytes.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn hash128(&self) -> Result<[u8; 16], DecodeError> {
        self.array(Type::Hash128)
    }

    /// A `Hash160`'s bytes.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn hash160(&self) -> Result<[u8; 20], DecodeError> {
        self.array(Type::Hash160)
    }

    /// A `Hash256`'s bytes.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn hash256(&self) -> Result<[u8; 32], DecodeError> {
        self.array(Type::Hash256)
    }

    /// A blob's bytes.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn blob(&self) -> Result<&'a [u8], DecodeError> {
        self.of(Type::Blob)
    }

    /// An account, or `None` for an account field left empty.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn account(&self) -> Result<Option<AccountId>, DecodeError> {
        let value = self.of(Type::AccountId)?;
        if value.is_empty() {
            return Ok(None);
        }
        Ok(Some(AccountId(
            value.try_into().map_err(|_| DecodeError::Value)?,
        )))
    }

    /// A `Vector256`'s hashes.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn vector256(&self) -> Result<&'a [[u8; 32]], DecodeError> {
        let (hashes, rest) = self.of(Type::Vector256)?.as_chunks::<32>();
        if !rest.is_empty() {
            return Err(DecodeError::Value);
        }
        Ok(hashes)
    }

    /// An amount of XRP, not below zero, or of a token.
    ///
    /// # Errors
    ///
    /// It is of another type, or another amount: of XRP below zero, or of
    /// an MPT ([`DecodeError::Value`]).
    pub fn amount(&self) -> Result<Amount, DecodeError> {
        let value = self.of(Type::Amount)?;
        if let Ok(bytes) = <[u8; 48]>::try_from(value) {
            return TokenAmount::from_bytes(&bytes)
                .map(Amount::Token)
                .ok_or(DecodeError::Value);
        }
        let bits = u64::from_be_bytes(value.try_into().map_err(|_| DecodeError::Value)?);
        if bits & (AMOUNT_NOT_XRP | AMOUNT_POSITIVE | AMOUNT_MPT) != AMOUNT_POSITIVE {
            return Err(DecodeError::Value);
        }
        let drops = bits & !AMOUNT_POSITIVE;
        if drops > crate::MAX_DROPS {
            return Err(DecodeError::Value);
        }
        Ok(Amount::Xrp(drops))
    }

    /// An object's fields.
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn fields(&self) -> Result<Fields<'a>, DecodeError> {
        Ok(Fields::nested(
            self.of(Type::Object)?,
            Level::Object,
            self.depth + 1,
        ))
    }

    /// An array's objects, each a field of type [`Type::Object`].
    ///
    /// # Errors
    ///
    /// It is of another type.
    pub fn elements(&self) -> Result<Fields<'a>, DecodeError> {
        Ok(Fields::nested(
            self.of(Type::Array)?,
            Level::Array,
            self.depth + 1,
        ))
    }
}

/// What holds the fields read: an object, whose fields ascend, or an
/// array, whose fields are objects in any order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Object,
    Array,
}

/// The fields of one object, or the objects of one array, in the order
/// they are written. Each is checked, its contents too, before it is
/// handed out; after an error, none follows.
#[derive(Clone, Debug)]
pub struct Fields<'a> {
    rest: &'a [u8],
    level: Level,
    depth: usize,
    last: Option<FieldId>,
    failed: bool,
}

impl<'a> Fields<'a> {
    /// The fields of a serialization: an object without its end marker,
    /// such as a validation, a manifest, a ledger object or a transaction.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self::nested(bytes, Level::Object, 1)
    }

    const fn nested(bytes: &'a [u8], level: Level, depth: usize) -> Self {
        Self {
            rest: bytes,
            level,
            depth,
            last: None,
            failed: false,
        }
    }

    fn read(&mut self) -> Result<Field<'a>, DecodeError> {
        let (id, header_bytes) = header(self.rest)?;
        if id == OBJECT_END || id == ARRAY_END || self.depth > MAX_DEPTH {
            return Err(DecodeError::Field);
        }
        match self.level {
            Level::Object if self.last.is_some_and(|last| id <= last) => {
                return Err(DecodeError::Field);
            }
            Level::Array if id.kind != Type::Object => return Err(DecodeError::Field),
            _ => {}
        }
        self.last = Some(id);
        let body = self
            .rest
            .get(header_bytes..)
            .ok_or(DecodeError::Truncated)?;
        let (start, end, consumed) = if id.kind.is_container() {
            let (end, consumed) = container(body, id.kind, self.depth)?;
            (0, end, consumed)
        } else {
            let (start, end) = value(id.kind, body)?;
            (start, end, end)
        };
        let value = body.get(start..end).ok_or(DecodeError::Truncated)?;
        self.rest = body.get(consumed..).ok_or(DecodeError::Truncated)?;
        Ok(Field {
            id,
            value,
            depth: self.depth,
        })
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<Field<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.rest.is_empty() {
            return None;
        }
        let field = self.read();
        self.failed = field.is_err();
        Some(field)
    }
}

/// Whether `bytes` are a serialization in canonical form, to the last
/// byte.
///
/// # Errors
///
/// Why they are not.
pub fn check(bytes: &[u8]) -> Result<(), DecodeError> {
    Fields::new(bytes).try_for_each(|field| field.map(drop))
}

/// The field whose header starts `bytes`, and the header's length, if it is
/// written in its shortest form: a code below 16 in a byte of its own is
/// not.
fn header(bytes: &[u8]) -> Result<(FieldId, usize), DecodeError> {
    let first = *bytes.first().ok_or(DecodeError::Truncated)?;
    let byte = |index: usize| bytes.get(index).copied().ok_or(DecodeError::Truncated);
    let ((kind, code), length) = match (first >> 4, first & 0x0f) {
        (0, 0) => ((byte(1)?, byte(2)?), 3),
        (0, code) => ((byte(1)?, code), 2),
        (kind, 0) => ((kind, byte(1)?), 2),
        (kind, code) => ((kind, code), 1),
    };
    let long = |nibble: u8, value: u8| nibble != 0 || value >= 16;
    if !long(first >> 4, kind) || !long(first & 0x0f, code) {
        return Err(DecodeError::Field);
    }
    let kind = Type::from_code(kind).ok_or(DecodeError::Field)?;
    Ok((FieldId { kind, code }, length))
}

/// An object or array being read: what it is, and in an object, the last
/// field read.
#[derive(Clone, Copy)]
struct Frame {
    level: Level,
    last: Option<FieldId>,
}

/// Where the contents of an object or array at the start of `bytes` end,
/// and where its end marker ends; `depth` is the container's own. Every
/// field within is checked, at most [`MAX_DEPTH`] deep, without recursion.
fn container(bytes: &[u8], kind: Type, depth: usize) -> Result<(usize, usize), DecodeError> {
    let level = |kind: Type| {
        if kind == Type::Array {
            Level::Array
        } else {
            Level::Object
        }
    };
    let mut frames = [Frame {
        level: Level::Object,
        last: None,
    }; MAX_DEPTH];
    *frames.first_mut().ok_or(DecodeError::Field)? = Frame {
        level: level(kind),
        last: None,
    };
    let mut open = 1;
    let mut at = 0;
    // Each turn reads a header of at least a byte.
    loop {
        let (id, header_bytes) = header(bytes.get(at..).ok_or(DecodeError::Truncated)?)?;
        let marker = at;
        at += header_bytes;
        let frame = frames.get_mut(open - 1).ok_or(DecodeError::Field)?;
        let end = match frame.level {
            Level::Object => OBJECT_END,
            Level::Array => ARRAY_END,
        };
        if id == end {
            open -= 1;
            if open == 0 {
                return Ok((marker, at));
            }
            continue;
        }
        if id == OBJECT_END || id == ARRAY_END || depth + open > MAX_DEPTH {
            return Err(DecodeError::Field);
        }
        match frame.level {
            Level::Object if frame.last.is_some_and(|last| id <= last) => {
                return Err(DecodeError::Field);
            }
            Level::Array if id.kind != Type::Object => return Err(DecodeError::Field),
            _ => frame.last = Some(id),
        }
        if id.kind.is_container() {
            // `depth + open` is at most MAX_DEPTH, and `depth` at least 1.
            *frames.get_mut(open).ok_or(DecodeError::Field)? = Frame {
                level: level(id.kind),
                last: None,
            };
            open += 1;
        } else {
            let (_, end) = value(id.kind, bytes.get(at..).ok_or(DecodeError::Truncated)?)?;
            at += end;
        }
    }
}

/// Where the value of a field of type `kind` at the start of `bytes`
/// starts, after any length prefix, and ends, if it is in canonical form.
/// Not for objects and arrays.
fn value(kind: Type, bytes: &[u8]) -> Result<(usize, usize), DecodeError> {
    let (start, length) = match kind {
        Type::Amount => (0, amount_bytes(bytes)?),
        Type::Issue => (0, issue_bytes(bytes)?),
        Type::PathSet => (0, path_set_bytes(bytes)?),
        kind if kind.has_length_prefix() => {
            let (prefix, length) = length_prefix(bytes)?;
            let fits = match kind {
                Type::AccountId => length == 0 || length == 20,
                Type::Vector256 => length.is_multiple_of(32),
                _ => true,
            };
            if !fits {
                return Err(DecodeError::Value);
            }
            (prefix, length)
        }
        kind => (0, kind.fixed_bytes().ok_or(DecodeError::Field)?),
    };
    let end = start + length;
    if end > bytes.len() {
        return Err(DecodeError::Truncated);
    }
    Ok((start, end))
}

/// A length prefix's length, and the length it writes.
fn length_prefix(bytes: &[u8]) -> Result<(usize, usize), DecodeError> {
    let byte = |index: usize| {
        bytes
            .get(index)
            .copied()
            .map(usize::from)
            .ok_or(DecodeError::Truncated)
    };
    let first = byte(0)?;
    match first {
        0..=192 => Ok((1, first)),
        193..=240 => Ok((2, 193 + ((first - 193) << 8) + byte(1)?)),
        241..=254 => {
            let length = 12_481 + ((first - 241) << 16) + (byte(1)? << 8) + byte(2)?;
            if length > MAX_VL_BYTES {
                return Err(DecodeError::Value);
            }
            Ok((3, length))
        }
        _ => Err(DecodeError::Value),
    }
}

/// An amount's length, if it is in the form rippled writes: XRP not a
/// negative zero; a token's value normalized, of a currency and an issuer
/// that are not XRP's; an MPT with no other bits in its first byte.
fn amount_bytes(bytes: &[u8]) -> Result<usize, DecodeError> {
    let head: [u8; 8] = bytes
        .get(..8)
        .ok_or(DecodeError::Truncated)?
        .try_into()
        .map_err(|_| DecodeError::Truncated)?;
    let bits = u64::from_be_bytes(head);
    if bits & AMOUNT_NOT_XRP != 0 {
        let body = bytes.get(..48).ok_or(DecodeError::Truncated)?;
        let currency = body.get(8..28).ok_or(DecodeError::Truncated)?;
        let issuer = body.get(28..48).ok_or(DecodeError::Truncated)?;
        let zero = |bytes: &[u8]| bytes.iter().all(|byte| *byte == 0);
        if TokenValue::from_bits(bits).is_none() || zero(currency) || zero(issuer) {
            return Err(DecodeError::Value);
        }
        Ok(48)
    } else if bits & AMOUNT_MPT != 0 {
        if ((bits >> 56) & !0x40) != 0x20 {
            return Err(DecodeError::Value);
        }
        Ok(33)
    } else {
        if bits == 0 {
            return Err(DecodeError::Value);
        }
        Ok(8)
    }
}

/// An issue's length: XRP, a currency and an issuer that is not XRP's, or
/// an MPT.
fn issue_bytes(bytes: &[u8]) -> Result<usize, DecodeError> {
    let zero = |bytes: &[u8]| bytes.iter().all(|byte| *byte == 0);
    if zero(bytes.get(..20).ok_or(DecodeError::Truncated)?) {
        return Ok(20);
    }
    let account = bytes.get(20..40).ok_or(DecodeError::Truncated)?;
    if account == NO_ACCOUNT {
        return Ok(44);
    }
    if zero(account) {
        return Err(DecodeError::Value);
    }
    Ok(40)
}

/// A path set's length, with the byte that ends it: paths of at least one
/// element, apart, each element's kind only what it holds, and its
/// account and issuer, where it names them, not XRP's.
fn path_set_bytes(bytes: &[u8]) -> Result<usize, DecodeError> {
    let zero = |bytes: &[u8]| bytes.iter().all(|byte| *byte == 0);
    let mut at = 0;
    let mut elements = 0;
    // Each turn reads a byte at least.
    loop {
        let kind = *bytes.get(at).ok_or(DecodeError::Truncated)?;
        at += 1;
        if kind == PATH_END || kind == PATH_BOUNDARY {
            if elements == 0 {
                return Err(DecodeError::Value);
            }
            if kind == PATH_END {
                return Ok(at);
            }
            elements = 0;
            continue;
        }
        if kind & !(PATH_ACCOUNT | PATH_CURRENCY | PATH_ISSUER | PATH_MPT) != 0
            || kind & (PATH_CURRENCY | PATH_MPT) == PATH_CURRENCY | PATH_MPT
        {
            return Err(DecodeError::Value);
        }
        for (part, length, may_be_zero) in [
            (PATH_ACCOUNT, 20, false),
            (PATH_CURRENCY, 20, true),
            (PATH_MPT, 24, true),
            (PATH_ISSUER, 20, false),
        ] {
            if kind & part != 0 {
                let bytes = bytes.get(at..at + length).ok_or(DecodeError::Truncated)?;
                if !may_be_zero && zero(bytes) {
                    return Err(DecodeError::Value);
                }
                at += length;
            }
        }
        elements += 1;
    }
}

/// Writes a field's header, in its shortest form.
pub fn write_header(out: &mut Vec<u8>, id: FieldId) {
    let (kind, code) = (id.kind.code(), id.code);
    match (kind < 16, code < 16) {
        (true, true) => out.push(kind << 4 | code),
        (true, false) => out.extend_from_slice(&[kind << 4, code]),
        (false, true) => out.extend_from_slice(&[code, kind]),
        (false, false) => out.extend_from_slice(&[0, kind, code]),
    }
}

/// Writes a field: its header, its value with a length prefix where its
/// type has one, and an object's or array's end marker. The value is as
/// [`Field::value`] holds it. A value that is not one of its type is a
/// bug.
pub fn write_field(out: &mut Vec<u8>, id: FieldId, value: &[u8]) {
    write_header(out, id);
    if id.kind.has_length_prefix() {
        write_length(out, value.len());
    } else if !id.kind.is_container() {
        let length = self::value(id.kind, value).map(|(_, end)| end);
        assert!(length == Ok(value.len()), "one value of its type");
    }
    out.extend_from_slice(value);
    match id.kind {
        Type::Object => write_header(out, OBJECT_END),
        Type::Array => write_header(out, ARRAY_END),
        _ => {}
    }
}

/// An amount of XRP, not below zero and at most all there is (a bug
/// otherwise), as a field's value.
#[must_use]
pub fn xrp(drops: u64) -> [u8; 8] {
    assert!(drops <= crate::MAX_DROPS, "XRP there is");
    (AMOUNT_POSITIVE | drops).to_be_bytes()
}

/// Writes a length prefix, in the form its length needs.
fn write_length(out: &mut Vec<u8>, length: usize) {
    assert!(length <= MAX_VL_BYTES, "lengths a prefix writes");
    let byte = |value: usize| u8::try_from(value & 0xff).unwrap_or(0);
    if length <= MAX_VL_ONE_BYTE {
        out.push(byte(length));
    } else if length <= MAX_VL_TWO_BYTES {
        let rest = length - 193;
        out.extend_from_slice(&[byte(193 + (rest >> 8)), byte(rest)]);
    } else {
        let rest = length - (MAX_VL_TWO_BYTES + 1);
        out.extend_from_slice(&[byte(241 + (rest >> 16)), byte(rest >> 8), byte(rest)]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use alloc::vec;

    use super::*;

    const UINT32: FieldId = FieldId::new(Type::UInt32, 2);
    const OBJECT: FieldId = FieldId::new(Type::Object, 10);
    const ARRAY: FieldId = FieldId::new(Type::Array, 9);

    fn read(bytes: &[u8]) -> Result<Vec<(FieldId, Vec<u8>)>, DecodeError> {
        Fields::new(bytes)
            .map(|field| field.map(|field| (field.id, field.value.to_vec())))
            .collect()
    }

    fn field(id: FieldId, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_field(&mut out, id, value);
        out
    }

    #[test]
    fn every_type_code_round_trips() {
        for code in 0..=u8::MAX {
            if let Some(kind) = Type::from_code(code) {
                assert_eq!(kind.code(), code);
            }
        }
        assert_eq!(Type::from_code(12), None);
        assert_eq!(Type::from_code(25), None);
        assert_eq!(Type::from_code(27), None);
    }

    #[test]
    fn headers_are_read_only_in_their_shortest_form() {
        assert_eq!(header(&[0x24]), Ok((FieldId::new(Type::UInt32, 4), 1)));
        assert_eq!(header(&[0x20, 0x04]), Err(DecodeError::Field));
        assert_eq!(header(&[0x04, 0x02]), Err(DecodeError::Field));
        assert_eq!(header(&[0x00, 0x02, 0x04]), Err(DecodeError::Field));
        assert_eq!(
            header(&[0x20, 0x1B]),
            Ok((FieldId::new(Type::UInt32, 27), 2))
        );
        assert_eq!(header(&[0x01, 0x10]), Ok((FieldId::new(Type::UInt8, 1), 2)));
        assert_eq!(
            header(&[0x00, 0x13, 0x10]),
            Ok((FieldId::new(Type::Vector256, 16), 3))
        );
        // Types the ledger has not.
        assert_eq!(header(&[0xC1]), Err(DecodeError::Field));
        assert_eq!(header(&[0x01, 0x19]), Err(DecodeError::Field));
        assert_eq!(header(&[0x20]), Err(DecodeError::Truncated));
        for id in [UINT32, OBJECT, FieldId::new(Type::Currency, 200)] {
            let mut out = Vec::new();
            write_header(&mut out, id);
            assert_eq!(header(&out), Ok((id, out.len())));
        }
    }

    #[test]
    fn length_prefixes_round_trip_at_their_edges() {
        for length in [0, 192, 193, 12_480, 12_481, MAX_VL_BYTES] {
            let mut out = Vec::new();
            write_length(&mut out, length);
            assert_eq!(length_prefix(&out), Ok((out.len(), length)));
        }
        // Past what a prefix of three bytes writes, and no prefix at all.
        assert_eq!(length_prefix(&[254, 0xFF, 0xFF]), Err(DecodeError::Value));
        assert_eq!(length_prefix(&[255]), Err(DecodeError::Value));
        assert_eq!(length_prefix(&[193]), Err(DecodeError::Truncated));
    }

    #[test]
    fn fields_ascend_once_each() {
        let one = field(UINT32, &[0, 0, 0, 1]);
        let blob = field(FieldId::new(Type::Blob, 3), b"key");
        let both = [one.clone(), blob.clone()].concat();
        assert_eq!(read(&both).unwrap().len(), 2);
        assert_eq!(read(&[blob, one.clone()].concat()), Err(DecodeError::Field));
        assert_eq!(read(&[one.clone(), one].concat()), Err(DecodeError::Field));
        assert_eq!(read(&both[..both.len() - 1]), Err(DecodeError::Truncated));
        assert_eq!(read(&[]), Ok(Vec::new()));
    }

    #[test]
    fn values_are_only_in_their_one_form() {
        let read_one = |id: FieldId, value: &[u8]| {
            let mut bytes = Vec::new();
            write_header(&mut bytes, id);
            bytes.extend_from_slice(value);
            check(&bytes)
        };
        let amount = FieldId::new(Type::Amount, 1);
        // XRP: positive, or negative but not zero.
        assert_eq!(
            read_one(amount, &0x4000_0000_0000_0001_u64.to_be_bytes()),
            Ok(())
        );
        assert_eq!(read_one(amount, &1_u64.to_be_bytes()), Ok(()));
        assert_eq!(
            read_one(amount, &0_u64.to_be_bytes()),
            Err(DecodeError::Value)
        );
        // A token: its value normalized, its currency and issuer not XRP's.
        let mut token = [1; 48];
        token[..8].copy_from_slice(&0x8000_0000_0000_0000_u64.to_be_bytes());
        assert_eq!(read_one(amount, &token), Ok(()));
        token[..8].copy_from_slice(&0xC000_0000_0000_0000_u64.to_be_bytes());
        assert_eq!(read_one(amount, &token), Err(DecodeError::Value));
        token[..8].copy_from_slice(&0xD4C3_8D7E_A4C6_8000_u64.to_be_bytes());
        assert_eq!(read_one(amount, &token), Ok(()));
        token[8..28].fill(0);
        assert_eq!(read_one(amount, &token), Err(DecodeError::Value));
        // An MPT: its first byte only its kind and sign.
        let mut mpt = [0; 33];
        mpt[0] = 0x60;
        assert_eq!(read_one(amount, &mpt), Ok(()));
        mpt[0] = 0x61;
        assert_eq!(read_one(amount, &mpt), Err(DecodeError::Value));
        // An account of 20 bytes or none, hashes in whole.
        let account = FieldId::new(Type::AccountId, 1);
        assert_eq!(check(&field(account, &[7; 20])), Ok(()));
        assert_eq!(check(&field(account, &[])), Ok(()));
        assert_eq!(read_one(account, &[19; 20]), Err(DecodeError::Value));
        let hashes = FieldId::new(Type::Vector256, 2);
        assert_eq!(check(&field(hashes, &[1; 64])), Ok(()));
        assert_eq!(read_one(hashes, &[33; 34]), Err(DecodeError::Value));
    }

    #[test]
    fn issues_are_xrp_a_token_or_an_mpt() {
        let issue = FieldId::new(Type::Issue, 3);
        assert_eq!(check(&field(issue, &[0; 20])), Ok(()));
        assert_eq!(check(&field(issue, &[1; 40])), Ok(()));
        let mut mpt = [1; 44];
        mpt[20..40].copy_from_slice(&NO_ACCOUNT);
        assert_eq!(check(&field(issue, &mpt)), Ok(()));
        // A currency with XRP's issuer.
        let mut bytes = Vec::new();
        write_header(&mut bytes, issue);
        bytes.extend_from_slice(&[1; 20]);
        bytes.extend_from_slice(&[0; 20]);
        assert_eq!(check(&bytes), Err(DecodeError::Value));
    }

    #[test]
    fn path_sets_hold_paths_of_elements() {
        let paths = FieldId::new(Type::PathSet, 1);
        let element = |kind: u8, parts: usize| {
            let mut bytes = vec![kind];
            bytes.extend(core::iter::repeat_n(9, parts));
            bytes
        };
        let account = element(PATH_ACCOUNT, 20);
        let asset = element(PATH_CURRENCY | PATH_ISSUER, 40);
        let set = [account.clone(), vec![PATH_BOUNDARY], asset.clone(), vec![0]].concat();
        assert_eq!(check(&field(paths, &set)), Ok(()));
        let read_one = |value: &[u8]| {
            let mut bytes = Vec::new();
            write_header(&mut bytes, paths);
            bytes.extend_from_slice(value);
            check(&bytes)
        };
        // An empty path, an unknown kind, both a currency and an MPT, an
        // account of zeros, or no end.
        assert_eq!(read_one(&[0]), Err(DecodeError::Value));
        assert_eq!(
            read_one(&[account.clone(), vec![PATH_BOUNDARY, 0]].concat()),
            Err(DecodeError::Value)
        );
        assert_eq!(
            read_one(&[element(0x02, 0), vec![0]].concat()),
            Err(DecodeError::Value)
        );
        assert_eq!(
            read_one(&[element(PATH_CURRENCY | PATH_MPT, 44), vec![0]].concat()),
            Err(DecodeError::Value)
        );
        let mut zero = element(PATH_ACCOUNT, 0);
        zero.extend_from_slice(&[0; 20]);
        zero.push(0);
        assert_eq!(read_one(&zero), Err(DecodeError::Value));
        assert_eq!(read_one(&account), Err(DecodeError::Truncated));
    }

    /// Objects nested `depth` deep, with a field innermost.
    fn nested(depth: usize) -> Vec<u8> {
        let mut bytes = field(UINT32, &[0; 4]);
        for _ in 0..depth {
            bytes = field(OBJECT, &bytes);
        }
        bytes
    }

    #[test]
    fn objects_and_arrays_nest_within_the_bound() {
        assert_eq!(check(&nested(MAX_DEPTH - 1)), Ok(()));
        assert_eq!(check(&nested(MAX_DEPTH)), Err(DecodeError::Field));
        // An empty object at the deepest depth holds no field too deep.
        let mut empty = field(OBJECT, &[]);
        for _ in 1..MAX_DEPTH {
            empty = field(OBJECT, &empty);
        }
        assert_eq!(check(&empty), Ok(()));
        // An array of objects, read element by element.
        let element = field(OBJECT, &field(UINT32, &[0, 0, 0, 7]));
        let array = field(ARRAY, &[element.clone(), element.clone()].concat());
        let fields = read(&array).unwrap();
        assert_eq!(fields.len(), 1);
        let outer = Fields::new(&array).next().unwrap().unwrap();
        let mut count = 0;
        for element in outer.elements().unwrap() {
            let inner = element.unwrap().fields().unwrap().next().unwrap().unwrap();
            assert_eq!(inner.uint32(), Ok(7));
            count += 1;
        }
        assert_eq!(count, 2);
    }

    #[test]
    fn end_markers_end_only_what_they_end() {
        let one = field(UINT32, &[0; 4]);
        let mut object_end = Vec::new();
        write_header(&mut object_end, OBJECT_END);
        let mut array_end = Vec::new();
        write_header(&mut array_end, ARRAY_END);
        // Not at the top.
        assert_eq!(check(&object_end), Err(DecodeError::Field));
        assert_eq!(
            check(&[one.clone(), array_end.clone()].concat()),
            Err(DecodeError::Field)
        );
        // An array that holds a field that is no object, or an object's end.
        let mut bytes = Vec::new();
        write_header(&mut bytes, ARRAY);
        bytes.extend_from_slice(&one);
        bytes.extend_from_slice(&array_end);
        assert_eq!(check(&bytes), Err(DecodeError::Field));
        let mut bytes = Vec::new();
        write_header(&mut bytes, ARRAY);
        bytes.extend_from_slice(&object_end);
        assert_eq!(check(&bytes), Err(DecodeError::Field));
        // An object ended as an array, or not ended.
        let mut bytes = Vec::new();
        write_header(&mut bytes, OBJECT);
        bytes.extend_from_slice(&one);
        assert_eq!(
            check(&[bytes.clone(), array_end].concat()),
            Err(DecodeError::Field)
        );
        assert_eq!(check(&bytes), Err(DecodeError::Truncated));
    }

    #[test]
    fn readers_take_only_their_type() {
        let bytes = field(UINT32, &[0, 0, 1, 0]);
        let one = Fields::new(&bytes).next().unwrap().unwrap();
        assert_eq!(one.uint32(), Ok(256));
        assert_eq!(one.uint64(), Err(DecodeError::Value));
        assert_eq!(one.blob(), Err(DecodeError::Value));
        assert!(one.fields().is_err());
        let amount = field(
            FieldId::new(Type::Amount, 8),
            &0x4000_0000_0000_000A_u64.to_be_bytes(),
        );
        let fee = Fields::new(&amount).next().unwrap().unwrap();
        assert_eq!(fee.amount(), Ok(Amount::Xrp(10)));
        let negative = field(FieldId::new(Type::Amount, 8), &10_u64.to_be_bytes());
        let fee = Fields::new(&negative).next().unwrap().unwrap();
        assert_eq!(fee.amount(), Err(DecodeError::Value));
    }
}
