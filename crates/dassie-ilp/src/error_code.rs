use core::fmt;

/// An ILP reject code: three IA5 characters, a class letter and two digits
/// by convention (ILPv4, RFC 27 §"Error Codes").
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCode([u8; 3]);

/// What a rejection says about retrying (RFC 27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    /// `F`: retrying the same packet will not help.
    Final,
    /// `T`: retrying later may succeed.
    Temporary,
    /// `R`: the packet's parameters relative to the path were wrong.
    Relative,
    /// Any other first character. Treated like a final error by callers.
    Unknown,
}

/// The three octets of a code literal, checked at compile time.
// Evaluated when the crate builds, for the code constants below.
#[allow(clippy::panic)]
const fn code_bytes(code: &str) -> [u8; 3] {
    match code.as_bytes() {
        [a, b, c] => [*a, *b, *c],
        _ => panic!("error codes have three characters"),
    }
}

macro_rules! codes {
    ($($constant:ident = $code:literal, $name:literal;)*) => {
        impl ErrorCode {
            $(
                #[doc = concat!("`", $code, "` ", $name, ".")]
                pub const $constant: Self = Self(code_bytes($code));
            )*

            /// The RFC 27 name of the code, or "Unknown Error".
            #[must_use]
            pub const fn name(self) -> &'static str {
                let [a, b, c] = self.0;
                $(
                    if matches!(code_bytes($code), [x, y, z] if x == a && y == b && z == c) {
                        return $name;
                    }
                )*
                "Unknown Error"
            }
        }
    };
}

codes! {
    F00_BAD_REQUEST = "F00", "Bad Request";
    F01_INVALID_PACKET = "F01", "Invalid Packet";
    // The original called F02 "Unavailable"; RFC 27 says "Unreachable".
    F02_UNREACHABLE = "F02", "Unreachable";
    F03_INVALID_AMOUNT = "F03", "Invalid Amount";
    F04_INSUFFICIENT_DESTINATION_AMOUNT = "F04", "Insufficient Destination Amount";
    F05_WRONG_CONDITION = "F05", "Wrong Condition";
    F06_UNEXPECTED_PAYMENT = "F06", "Unexpected Payment";
    F07_CANNOT_RECEIVE = "F07", "Cannot Receive";
    F08_AMOUNT_TOO_LARGE = "F08", "Amount Too Large";
    F99_APPLICATION_ERROR = "F99", "Application Error";
    T00_INTERNAL_ERROR = "T00", "Internal Error";
    T01_PEER_UNREACHABLE = "T01", "Peer Unreachable";
    T02_PEER_BUSY = "T02", "Peer Busy";
    T03_CONNECTOR_BUSY = "T03", "Connector Busy";
    T04_INSUFFICIENT_LIQUIDITY = "T04", "Insufficient Liquidity";
    T05_RATE_LIMITED = "T05", "Rate Limited";
    T99_APPLICATION_ERROR = "T99", "Application Error";
    R00_TRANSFER_TIMED_OUT = "R00", "Transfer Timed Out";
    R01_INSUFFICIENT_SOURCE_AMOUNT = "R01", "Insufficient Source Amount";
    R02_INSUFFICIENT_TIMEOUT = "R02", "Insufficient Timeout";
    R99_APPLICATION_ERROR = "R99", "Application Error";
}

impl ErrorCode {
    /// Wraps three code characters, which must be 7-bit ASCII so the code
    /// fits its IA5String type.
    #[must_use]
    pub const fn new(code: [u8; 3]) -> Option<Self> {
        let [a, b, c] = code;
        if a < 0x80 && b < 0x80 && c < 0x80 {
            Some(Self(code))
        } else {
            None
        }
    }

    /// The three code characters.
    #[must_use]
    pub const fn bytes(self) -> [u8; 3] {
        self.0
    }

    /// The code as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Construction guarantees 7-bit ASCII.
        core::str::from_utf8(&self.0).unwrap_or("???")
    }

    /// The class the first character indicates.
    #[must_use]
    pub const fn class(self) -> ErrorClass {
        match self.0[0] {
            b'F' => ErrorClass::Final,
            b'T' => ErrorClass::Temporary,
            b'R' => ErrorClass::Relative,
            _ => ErrorClass::Unknown,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ErrorCode({} {})", self.as_str(), self.name())
    }
}
