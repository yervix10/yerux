// Numeric types shared by the front end and the backends.
//
// `Int` (= `Int64`) stays `Ty::Int`; every other number type is `Ty::Num(Nk)`:
//   Int8 Int16 Int32 UInt8 UInt16 UInt32 UInt64   (aliases i8 i16 i32 u8 u16 u32 u64; `Int64` = `Int`)
//   Float32 Float                                  (aliases f32 f64; `Float` is the 64-bit IEEE double)
// Representation in the native backends: every number is one 64-bit word.
//   * integers: the value, sign- or zero-extended to 64 bits (UInt64 uses the full bit pattern);
//   * Float: the IEEE-754 binary64 bit pattern; Float32: the binary32 bit pattern in the low 32 bits.
use crate::syntax::Ty;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Nk {
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
}

pub const ALL_NK: [Nk; 9] = [
    Nk::I8,
    Nk::I16,
    Nk::I32,
    Nk::U8,
    Nk::U16,
    Nk::U32,
    Nk::U64,
    Nk::F32,
    Nk::F64,
];

impl Nk {
    pub fn name(self) -> &'static str {
        match self {
            Nk::I8 => "Int8",
            Nk::I16 => "Int16",
            Nk::I32 => "Int32",
            Nk::U8 => "UInt8",
            Nk::U16 => "UInt16",
            Nk::U32 => "UInt32",
            Nk::U64 => "UInt64",
            Nk::F32 => "Float32",
            Nk::F64 => "Float",
        }
    }
    /// Short suffix used in literals (`200u8`, `1.5f32`) and in the names of prelude functions.
    pub fn suffix(self) -> &'static str {
        match self {
            Nk::I8 => "i8",
            Nk::I16 => "i16",
            Nk::I32 => "i32",
            Nk::U8 => "u8",
            Nk::U16 => "u16",
            Nk::U32 => "u32",
            Nk::U64 => "u64",
            Nk::F32 => "f32",
            Nk::F64 => "f64",
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Nk::F32 | Nk::F64)
    }
    pub fn signed(self) -> bool {
        matches!(self, Nk::I8 | Nk::I16 | Nk::I32 | Nk::F32 | Nk::F64)
    }
    pub fn bits(self) -> u32 {
        match self {
            Nk::I8 | Nk::U8 => 8,
            Nk::I16 | Nk::U16 => 16,
            Nk::I32 | Nk::U32 | Nk::F32 => 32,
            Nk::U64 | Nk::F64 => 64,
        }
    }
    pub fn cty(self) -> &'static str {
        match self {
            Nk::I8 => "int8_t",
            Nk::I16 => "int16_t",
            Nk::I32 => "int32_t",
            Nk::U8 => "uint8_t",
            Nk::U16 => "uint16_t",
            Nk::U32 => "uint32_t",
            Nk::U64 => "uint64_t",
            Nk::F32 => "float",
            Nk::F64 => "double",
        }
    }
}

/// Which numeric type does this name stand for (`Int8`, `f32`, `i64`, ...)? `Int`/`Int64`/`i64` is `Ty::Int`.
pub fn ty_from_name(n: &str) -> Option<Ty> {
    Some(match n {
        "Int64" | "i64" => Ty::Int,
        "Int8" | "i8" => Ty::Num(Nk::I8),
        "Int16" | "i16" => Ty::Num(Nk::I16),
        "Int32" | "i32" => Ty::Num(Nk::I32),
        "UInt8" | "u8" => Ty::Num(Nk::U8),
        "UInt16" | "u16" => Ty::Num(Nk::U16),
        "UInt32" | "u32" => Ty::Num(Nk::U32),
        "UInt64" | "u64" => Ty::Num(Nk::U64),
        "Float32" | "f32" => Ty::Num(Nk::F32),
        "Float" | "f64" => Ty::Num(Nk::F64),
        _ => return None,
    })
}

/// Names that cannot be used for user types.
pub const NUMERIC_TYPE_NAMES: &[&str] = &[
    "Int64", "Int8", "Int16", "Int32", "UInt8", "UInt16", "UInt32", "UInt64", "Float32", "Float",
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
];

/// Literal suffixes (`5u8`).
pub fn suffix_ty(s: &str) -> Option<Ty> {
    match s {
        "i64" | "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" => {
            ty_from_name(s)
        }
        _ => None,
    }
}

pub fn is_num(t: Ty) -> bool {
    matches!(t, Ty::Int | Ty::Range(..) | Ty::Num(_))
}
pub fn is_float(t: Ty) -> bool {
    matches!(t, Ty::Num(k) if k.is_float())
}
pub fn is_f32(t: Ty) -> bool {
    t == Ty::Num(Nk::F32)
}
/// An integer type (Int, Int8..UInt64).
pub fn is_int(t: Ty) -> bool {
    matches!(t, Ty::Int | Ty::Range(..)) || matches!(t, Ty::Num(k) if !k.is_float())
}
pub fn is_u64(t: Ty) -> bool {
    t == Ty::Num(Nk::U64)
}
pub fn is_signed_int(t: Ty) -> bool {
    match t {
        Ty::Int | Ty::Range(..) => true,
        Ty::Num(k) => !k.is_float() && k.signed(),
        _ => false,
    }
}
pub fn bits_of(t: Ty) -> u32 {
    match t {
        Ty::Num(k) => k.bits(),
        _ => 64,
    }
}
/// Inclusive value range of an integer type.
pub fn int_range(t: Ty) -> (i128, i128) {
    match t {
        Ty::Range(lo, hi) => (lo as i128, hi as i128),
        Ty::Num(Nk::I8) => (-128, 127),
        Ty::Num(Nk::I16) => (-32768, 32767),
        Ty::Num(Nk::I32) => (i32::MIN as i128, i32::MAX as i128),
        Ty::Num(Nk::U8) => (0, 255),
        Ty::Num(Nk::U16) => (0, 65535),
        Ty::Num(Nk::U32) => (0, u32::MAX as i128),
        Ty::Num(Nk::U64) => (0, u64::MAX as i128),
        _ => (i64::MIN as i128, i64::MAX as i128),
    }
}
/// Name of a numeric type in the `to_xxx()` conversion methods: `to_int`, `to_float`, `to_uint8`...
pub fn conv_method(t: Ty) -> String {
    match t {
        Ty::Int => "to_int".into(),
        Ty::Num(Nk::F64) => "to_float".into(),
        Ty::Num(k) => format!("to_{}", k.name().to_lowercase()),
        _ => String::new(),
    }
}
/// `Int8` -> `truncate_int8`.
pub fn trunc_method(t: Ty) -> String {
    match t {
        Ty::Int => "truncate_int".into(),
        Ty::Num(k) => format!("truncate_{}", k.name().to_lowercase()),
        _ => String::new(),
    }
}
/// Suffix of prelude function names for a numeric type: `i8`..`u64`, `f32`, `f64`; `Int` -> `i64`.
pub fn sfx(t: Ty) -> &'static str {
    match t {
        Ty::Int | Ty::Range(..) => "i64",
        Ty::Num(k) => k.suffix(),
        _ => "",
    }
}

/// All numeric method names that convert (`to_int8`, `truncate_uint16`, ...).
pub fn conv_target(m: &str) -> Option<(Ty, bool)> {
    let all: Vec<Ty> = std::iter::once(Ty::Int)
        .chain(ALL_NK.iter().map(|k| Ty::Num(*k)))
        .collect();
    for t in all {
        if m == conv_method(t) {
            return Some((t, false));
        }
        if !is_float(t) && m == trunc_method(t) {
            return Some((t, true));
        }
    }
    None
}
