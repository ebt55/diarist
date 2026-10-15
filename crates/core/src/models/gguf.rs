//! GGUF header reader: magic, version (2 or 3), tensor and key/value counts
//! and the key/value pairs. Tensor infos and data are never read, so a
//! 700 MB model costs a few hundred kilobytes of I/O at most.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::error::{CoreError, Result};

const MAGIC: &[u8; 4] = b"GGUF";
/// Longest string accepted (keys and values).
const MAX_STRING: u64 = 16 << 20;
/// Most key/value pairs accepted.
const MAX_KV: u64 = 1 << 20;
/// Arrays longer than this keep only their length (the elements are skipped).
const MAX_ARRAY_ITEMS: u64 = 4_096;
/// Longest array accepted at all.
const MAX_ARRAY_LEN: u64 = 1 << 32;

/// A metadata value.
#[derive(Debug, Clone, PartialEq)]
pub enum GgufValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    Bool(bool),
    Str(String),
    U64(u64),
    I64(i64),
    F64(f64),
    /// `items` is empty when the array is longer than 4,096 elements.
    Array {
        elem_type: u32,
        len: u64,
        items: Vec<GgufValue>,
    },
}

impl GgufValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            GgufValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        Some(match *self {
            GgufValue::U8(v) => v.into(),
            GgufValue::U16(v) => v.into(),
            GgufValue::U32(v) => v.into(),
            GgufValue::U64(v) => v,
            GgufValue::I8(v) => u64::try_from(v).ok()?,
            GgufValue::I16(v) => u64::try_from(v).ok()?,
            GgufValue::I32(v) => u64::try_from(v).ok()?,
            GgufValue::I64(v) => u64::try_from(v).ok()?,
            _ => return None,
        })
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            GgufValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[GgufValue]> {
        match self {
            GgufValue::Array { items, .. } => Some(items),
            _ => None,
        }
    }
}

/// The parsed header.
#[derive(Debug, Clone, PartialEq)]
pub struct GgufHeader {
    pub version: u32,
    pub tensor_count: u64,
    pub kv: BTreeMap<String, GgufValue>,
}

impl GgufHeader {
    pub fn get(&self, key: &str) -> Option<&GgufValue> {
        self.kv.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(GgufValue::as_str)
    }

    /// `general.architecture`.
    pub fn architecture(&self) -> Option<&str> {
        self.get_str("general.architecture")
    }

    /// `general.name`, cleaned: a leading `.` and a trailing `.gguf` are dropped.
    pub fn name(&self) -> Option<String> {
        let raw = self.get_str("general.name")?.trim();
        let s = raw.trim_start_matches('.');
        let s = s.strip_suffix(".gguf").unwrap_or(s).trim();
        (!s.is_empty()).then(|| s.to_owned())
    }

    /// `general.file_type` (llama.cpp's `LLAMA_FTYPE_*`).
    pub fn file_type(&self) -> Option<u32> {
        self.get("general.file_type")
            .and_then(GgufValue::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    }

    /// Quantization name from `general.file_type` (e.g. `Q8_0`).
    pub fn quant(&self) -> Option<String> {
        self.file_type().and_then(ftype_name).map(str::to_owned)
    }
}

/// llama.cpp file types.
pub fn ftype_name(ftype: u32) -> Option<&'static str> {
    Some(match ftype {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        32 => "BF16",
        _ => return None,
    })
}

/// Reads the header of the GGUF file at `path`.
pub fn read_header(path: &Path) -> Result<GgufHeader> {
    let f =
        File::open(path).map_err(|e| CoreError::Models(format!("open {}: {e}", path.display())))?;
    parse_header(BufReader::with_capacity(64 * 1024, f))
        .map_err(|e| CoreError::Models(format!("{}: {e}", path.display())))
}

/// Parses a GGUF header from any reader positioned at the file start.
pub fn parse_header<R: Read>(mut r: R) -> std::result::Result<GgufHeader, String> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)
        .map_err(|e| format!("read magic: {e}"))?;
    if &magic != MAGIC {
        return Err("not a GGUF file (bad magic)".into());
    }
    let version = read_u32(&mut r)?;
    if !(2..=3).contains(&version) {
        return Err(format!("unsupported GGUF version {version}"));
    }
    let tensor_count = read_u64(&mut r)?;
    let kv_count = read_u64(&mut r)?;
    if kv_count > MAX_KV {
        return Err(format!("implausible metadata count {kv_count}"));
    }
    let mut kv = BTreeMap::new();
    for _ in 0..kv_count {
        let key = read_string(&mut r)?;
        let ty = read_u32(&mut r)?;
        let value = read_value(&mut r, ty, 0)?;
        kv.insert(key, value);
    }
    Ok(GgufHeader {
        version,
        tensor_count,
        kv,
    })
}

fn read_exact<const N: usize, R: Read>(r: &mut R) -> std::result::Result<[u8; N], String> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)
        .map_err(|e| format!("truncated header: {e}"))?;
    Ok(b)
}

fn read_u32<R: Read>(r: &mut R) -> std::result::Result<u32, String> {
    Ok(u32::from_le_bytes(read_exact(r)?))
}

fn read_u64<R: Read>(r: &mut R) -> std::result::Result<u64, String> {
    Ok(u64::from_le_bytes(read_exact(r)?))
}

fn read_string<R: Read>(r: &mut R) -> std::result::Result<String, String> {
    let len = read_u64(r)?;
    if len > MAX_STRING {
        return Err(format!("implausible string length {len}"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)
        .map_err(|e| format!("truncated string: {e}"))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Size in bytes of a fixed-size element type, `None` for strings and arrays.
fn fixed_size(ty: u32) -> Option<u64> {
    Some(match ty {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => return None,
    })
}

fn read_value<R: Read>(r: &mut R, ty: u32, depth: u32) -> std::result::Result<GgufValue, String> {
    Ok(match ty {
        0 => GgufValue::U8(read_exact::<1, _>(r)?[0]),
        1 => GgufValue::I8(read_exact::<1, _>(r)?[0] as i8),
        2 => GgufValue::U16(u16::from_le_bytes(read_exact(r)?)),
        3 => GgufValue::I16(i16::from_le_bytes(read_exact(r)?)),
        4 => GgufValue::U32(u32::from_le_bytes(read_exact(r)?)),
        5 => GgufValue::I32(i32::from_le_bytes(read_exact(r)?)),
        6 => GgufValue::F32(f32::from_le_bytes(read_exact(r)?)),
        7 => GgufValue::Bool(read_exact::<1, _>(r)?[0] != 0),
        8 => GgufValue::Str(read_string(r)?),
        9 => {
            if depth > 4 {
                return Err("arrays nested too deeply".into());
            }
            let elem_type = read_u32(r)?;
            let len = read_u64(r)?;
            if len > MAX_ARRAY_LEN {
                return Err(format!("implausible array length {len}"));
            }
            let keep = len <= MAX_ARRAY_ITEMS;
            let mut items = Vec::new();
            match (keep, fixed_size(elem_type)) {
                (false, Some(size)) => {
                    let skip = len
                        .checked_mul(size)
                        .ok_or_else(|| "array too large".to_owned())?;
                    let copied = std::io::copy(&mut r.take(skip), &mut std::io::sink())
                        .map_err(|e| format!("truncated array: {e}"))?;
                    if copied != skip {
                        return Err("truncated array".into());
                    }
                }
                _ => {
                    for _ in 0..len {
                        let v = read_value(r, elem_type, depth + 1)?;
                        if keep {
                            items.push(v);
                        }
                    }
                }
            }
            GgufValue::Array {
                elem_type,
                len,
                items,
            }
        }
        10 => GgufValue::U64(u64::from_le_bytes(read_exact(r)?)),
        11 => GgufValue::I64(i64::from_le_bytes(read_exact(r)?)),
        12 => GgufValue::F64(f64::from_le_bytes(read_exact(r)?)),
        other => return Err(format!("unknown metadata type {other}")),
    })
}

/// Builds GGUF header bytes (tests and fixtures). Values are written with
/// their natural type; arrays of strings use type 8.
pub mod build {
    use super::GgufValue;

    pub fn header(version: u32, tensor_count: u64, kv: &[(&str, GgufValue)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(super::MAGIC);
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&tensor_count.to_le_bytes());
        out.extend_from_slice(&(kv.len() as u64).to_le_bytes());
        for (k, v) in kv {
            string(&mut out, k);
            out.extend_from_slice(&type_of(v).to_le_bytes());
            value(&mut out, v);
        }
        out
    }

    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn type_of(v: &GgufValue) -> u32 {
        match v {
            GgufValue::U8(_) => 0,
            GgufValue::I8(_) => 1,
            GgufValue::U16(_) => 2,
            GgufValue::I16(_) => 3,
            GgufValue::U32(_) => 4,
            GgufValue::I32(_) => 5,
            GgufValue::F32(_) => 6,
            GgufValue::Bool(_) => 7,
            GgufValue::Str(_) => 8,
            GgufValue::Array { .. } => 9,
            GgufValue::U64(_) => 10,
            GgufValue::I64(_) => 11,
            GgufValue::F64(_) => 12,
        }
    }

    fn value(out: &mut Vec<u8>, v: &GgufValue) {
        match v {
            GgufValue::U8(x) => out.push(*x),
            GgufValue::I8(x) => out.push(*x as u8),
            GgufValue::U16(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::I16(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::U32(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::I32(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::F32(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::Bool(x) => out.push(u8::from(*x)),
            GgufValue::Str(s) => string(out, s),
            GgufValue::U64(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::I64(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::F64(x) => out.extend_from_slice(&x.to_le_bytes()),
            GgufValue::Array {
                elem_type, items, ..
            } => {
                out.extend_from_slice(&elem_type.to_le_bytes());
                out.extend_from_slice(&(items.len() as u64).to_le_bytes());
                for i in items {
                    value(out, i);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> GgufValue {
        GgufValue::Str(v.into())
    }

    #[test]
    fn round_trips_a_minimal_header() {
        let bytes = build::header(
            3,
            657,
            &[
                ("general.architecture", s("asr")),
                (
                    "general.name",
                    s(".nemotron-3.5-asr-streaming-0.6b.q8_0.gguf"),
                ),
                ("general.file_type", GgufValue::U32(7)),
                ("asr.head_type", s("rnnt")),
                ("asr.encoder.cache_supported", GgufValue::Bool(true)),
                ("asr.preprocessor.window_size", GgufValue::F32(0.025)),
                ("x.i64", GgufValue::I64(-5)),
                (
                    "asr.rnnt.prompt_dictionary",
                    GgufValue::Array {
                        elem_type: 8,
                        len: 2,
                        items: vec![s("en-US:0"), s("de-DE:4")],
                    },
                ),
            ],
        );
        // Tensor data after the header is never touched.
        let mut with_junk = bytes.clone();
        with_junk.extend_from_slice(&[0xAB; 64]);
        let h = parse_header(&with_junk[..]).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.tensor_count, 657);
        assert_eq!(h.architecture(), Some("asr"));
        assert_eq!(
            h.name().as_deref(),
            Some("nemotron-3.5-asr-streaming-0.6b.q8_0")
        );
        assert_eq!(h.quant().as_deref(), Some("Q8_0"));
        assert_eq!(
            h.get("asr.encoder.cache_supported")
                .and_then(GgufValue::as_bool),
            Some(true)
        );
        assert_eq!(h.get("x.i64"), Some(&GgufValue::I64(-5)));
        let dict = h
            .get("asr.rnnt.prompt_dictionary")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(dict[1].as_str(), Some("de-DE:4"));
    }

    #[test]
    fn long_arrays_keep_only_their_length() {
        let big = GgufValue::Array {
            elem_type: 4,
            len: 5_000,
            items: (0..5_000).map(GgufValue::U32).collect(),
        };
        let strings = GgufValue::Array {
            elem_type: 8,
            len: 5_000,
            items: (0..5_000).map(|i| s(&format!("tok{i}"))).collect(),
        };
        let bytes = build::header(
            2,
            1,
            &[
                ("a", big),
                ("b", strings),
                ("general.architecture", s("sortformer")),
            ],
        );
        let h = parse_header(&bytes[..]).unwrap();
        assert_eq!(h.version, 2);
        assert!(
            matches!(h.get("a"), Some(GgufValue::Array { len: 5_000, items, .. }) if items.is_empty())
        );
        assert!(
            matches!(h.get("b"), Some(GgufValue::Array { len: 5_000, items, .. }) if items.is_empty())
        );
        assert_eq!(h.architecture(), Some("sortformer"));
    }

    #[test]
    fn rejects_bad_files() {
        assert!(parse_header(&b"GGML\x03\0\0\0"[..])
            .unwrap_err()
            .contains("magic"));
        let v1 = build::header(1, 0, &[]);
        assert!(parse_header(&v1[..]).unwrap_err().contains("version 1"));
        let mut truncated = build::header(3, 0, &[("general.name", s("abc"))]);
        truncated.truncate(truncated.len() - 2);
        assert!(parse_header(&truncated[..]).is_err());
        let mut huge = build::header(3, 0, &[]);
        huge[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse_header(&huge[..]).unwrap_err().contains("implausible"));
    }

    /// Reads every GGUF under `DIARIST_MODELS_DIR` (the real downloads).
    #[test]
    fn real_models_when_available() {
        let Some(dir) = std::env::var_os("DIARIST_MODELS_DIR") else {
            return;
        };
        let mut found = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "gguf") {
                    let h = read_header(&p).unwrap();
                    assert!(h.architecture().is_some(), "{}", p.display());
                    assert_eq!(h.quant().as_deref(), Some("Q8_0"), "{}", p.display());
                    found += 1;
                }
            }
        }
        assert!(found > 0, "no GGUF files found");
    }
}
