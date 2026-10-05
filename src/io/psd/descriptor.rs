//! Photoshop action descriptors and the text engine's `EngineData`, as found in layer blocks.
//!
//! Descriptors follow Adobe's *Photoshop File Formats Specification*, "Descriptor structure".
//! `EngineData` is a PostScript-like dictionary (`<< /Key value >>`, `[ ]`, `(string)`).
//! Both come from untrusted files, so recursion depth, item counts and every length are capped,
//! and nothing is preallocated from a declared count.
use anyhow::{Result, bail, ensure};

use super::Reader;

/// How deeply descriptors may nest (objects inside lists inside objects…).
pub(crate) const MAX_DESCRIPTOR_DEPTH: usize = 32;
/// Values read from one descriptor, across all levels.
const MAX_DESCRIPTOR_ITEMS: usize = 100_000;
/// How deeply `EngineData` dictionaries and arrays may nest.
pub(crate) const MAX_ENGINE_DEPTH: usize = 64;
/// Values read from one `EngineData` block.
const MAX_ENGINE_ITEMS: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Descriptor(Descriptor),
    List(Vec<Value>),
    Double(f64),
    Unit([u8; 4], f64),
    Text(String),
    Enum(Vec<u8>, Vec<u8>),
    Long(i32),
    Bool(bool),
    Raw(Vec<u8>),
    /// References, classes, aliases and float lists: read past, never interpreted.
    Other,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Descriptor {
    pub class: Vec<u8>,
    pub items: Vec<(Vec<u8>, Value)>,
}

impl Descriptor {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.items
            .iter()
            .find(|(k, _)| k == key.as_bytes())
            .map(|(_, v)| v)
    }

    pub fn double(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Value::Double(v) | Value::Unit(_, v) => Some(*v).filter(|v| v.is_finite()),
            Value::Long(v) => Some(f64::from(*v)),
            _ => None,
        }
    }

    pub fn long(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            Value::Long(v) => Some(*v),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Value::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Text(v) => Some(v),
            _ => None,
        }
    }

    /// The value of an enumerated item.
    pub fn enumeration(&self, key: &str) -> Option<&[u8]> {
        match self.get(key)? {
            Value::Enum(_, value) => Some(value),
            _ => None,
        }
    }

    pub fn object(&self, key: &str) -> Option<&Descriptor> {
        match self.get(key)? {
            Value::Descriptor(v) => Some(v),
            _ => None,
        }
    }

    pub fn list(&self, key: &str) -> Option<&[Value]> {
        match self.get(key)? {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn raw(&self, key: &str) -> Option<&[u8]> {
        match self.get(key)? {
            Value::Raw(v) => Some(v),
            _ => None,
        }
    }
}

/// Parse a descriptor (class name, class ID, items) starting at the reader's position.
pub(crate) fn descriptor(reader: &mut Reader<'_>) -> Result<Descriptor> {
    let mut parser = Parser { items: 0 };
    parser.descriptor(reader, 0)
}

/// Parse a "versioned" descriptor: a 4-byte descriptor version (16) and the descriptor.
pub(crate) fn versioned(reader: &mut Reader<'_>) -> Result<Descriptor> {
    ensure!(
        reader.u32()? == 16,
        "Unsupported Photoshop descriptor version"
    );
    descriptor(reader)
}

struct Parser {
    items: usize,
}

fn unicode(reader: &mut Reader<'_>) -> Result<String> {
    let count = reader.u32()? as usize;
    let bytes = reader.bytes(count.checked_mul(2).ok_or_else(super::damaged)?)?;
    Ok(super::utf16(bytes).trim_end_matches('\0').to_owned())
}

/// A key or class ID: a length, then that many bytes, or four bytes when the length is 0.
fn identifier(reader: &mut Reader<'_>) -> Result<Vec<u8>> {
    let length = reader.u32()? as usize;
    Ok(reader.bytes(if length == 0 { 4 } else { length })?.to_vec())
}

fn os_type(reader: &mut Reader<'_>) -> Result<[u8; 4]> {
    Ok(reader.bytes(4)?.try_into().unwrap())
}

impl Parser {
    fn count(&mut self) -> Result<()> {
        self.items += 1;
        ensure!(
            self.items <= MAX_DESCRIPTOR_ITEMS,
            "Photoshop descriptor is too large"
        );
        Ok(())
    }

    fn descriptor(&mut self, reader: &mut Reader<'_>, depth: usize) -> Result<Descriptor> {
        ensure!(
            depth < MAX_DESCRIPTOR_DEPTH,
            "Photoshop descriptor is nested too deeply"
        );
        unicode(reader)?;
        let class = identifier(reader)?;
        let count = reader.u32()?;
        let mut items = Vec::new();
        for _ in 0..count {
            self.count()?;
            let key = identifier(reader)?;
            let kind = os_type(reader)?;
            let value = self.value(reader, kind, depth)?;
            items.push((key, value));
        }
        Ok(Descriptor { class, items })
    }

    fn value(&mut self, reader: &mut Reader<'_>, kind: [u8; 4], depth: usize) -> Result<Value> {
        Ok(match &kind {
            b"Objc" | b"GlbO" => Value::Descriptor(self.descriptor(reader, depth + 1)?),
            b"VlLs" => {
                ensure!(
                    depth + 1 < MAX_DESCRIPTOR_DEPTH,
                    "Photoshop descriptor is nested too deeply"
                );
                let count = reader.u32()?;
                let mut values = Vec::new();
                for _ in 0..count {
                    self.count()?;
                    let kind = os_type(reader)?;
                    values.push(self.value(reader, kind, depth + 1)?);
                }
                Value::List(values)
            }
            b"doub" => Value::Double(reader.f64()?),
            b"UntF" => Value::Unit(os_type(reader)?, reader.f64()?),
            b"UnFl" => {
                os_type(reader)?;
                let count = reader.u32()? as usize;
                reader.skip(count.checked_mul(8).ok_or_else(super::damaged)?)?;
                Value::Other
            }
            b"TEXT" => Value::Text(unicode(reader)?),
            b"enum" => Value::Enum(identifier(reader)?, identifier(reader)?),
            b"long" => Value::Long(reader.i32()?),
            b"comp" => {
                reader.skip(8)?;
                Value::Other
            }
            b"bool" => Value::Bool(reader.u8()? != 0),
            b"type" | b"GlbC" => {
                unicode(reader)?;
                identifier(reader)?;
                Value::Other
            }
            b"alis" | b"Pth " => {
                let length = reader.u32()? as usize;
                reader.skip(length)?;
                Value::Other
            }
            b"tdta" => {
                let length = reader.u32()? as usize;
                Value::Raw(reader.bytes(length)?.to_vec())
            }
            b"obj " => {
                let count = reader.u32()?;
                for _ in 0..count {
                    self.count()?;
                    match &os_type(reader)? {
                        b"prop" => {
                            unicode(reader)?;
                            identifier(reader)?;
                            identifier(reader)?;
                        }
                        b"Clss" => {
                            unicode(reader)?;
                            identifier(reader)?;
                        }
                        b"Enmr" => {
                            unicode(reader)?;
                            identifier(reader)?;
                            identifier(reader)?;
                            identifier(reader)?;
                        }
                        b"rele" => {
                            unicode(reader)?;
                            identifier(reader)?;
                            reader.skip(4)?;
                        }
                        b"Idnt" | b"indx" => reader.skip(4)?,
                        b"name" => {
                            unicode(reader)?;
                            identifier(reader)?;
                            unicode(reader)?;
                        }
                        _ => bail!("Unsupported Photoshop reference"),
                    }
                }
                Value::Other
            }
            _ => bail!("Unsupported Photoshop descriptor value"),
        })
    }
}

/// A value from the text engine's `EngineData`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Engine {
    Dict(Vec<(String, Engine)>),
    Array(Vec<Engine>),
    Number(f64),
    Bool(bool),
    String(String),
    Name(String),
}

impl Engine {
    pub fn get(&self, key: &str) -> Option<&Engine> {
        match self {
            Self::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a path of dictionary keys.
    pub fn walk(&self, path: &[&str]) -> Option<&Engine> {
        path.iter().try_fold(self, |value, key| value.get(key))
    }

    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Number(v) => Some(*v).filter(|v| v.is_finite()),
            _ => None,
        }
    }

    pub fn boolean(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn string(&self) -> Option<&str> {
        match self {
            Self::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn array(&self) -> &[Engine] {
        match self {
            Self::Array(v) => v,
            _ => &[],
        }
    }
}

/// Parse `EngineData`: one top-level dictionary.
pub(crate) fn engine(data: &[u8]) -> Result<Engine> {
    let mut parser = EngineParser {
        data,
        pos: 0,
        items: 0,
    };
    parser.space();
    let value = parser.value(0)?;
    ensure!(
        matches!(value, Engine::Dict(_)),
        "Invalid Photoshop text data"
    );
    Ok(value)
}

struct EngineParser<'a> {
    data: &'a [u8],
    pos: usize,
    items: usize,
}

impl EngineParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn space(&mut self) {
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_whitespace() || b == 0)
        {
            self.pos += 1;
        }
    }

    fn starts(&self, token: &[u8]) -> bool {
        self.data
            .get(self.pos..)
            .is_some_and(|rest| rest.starts_with(token))
    }

    fn word(&mut self) -> &[u8] {
        let start = self.pos;
        while self.peek().is_some_and(|b| {
            !b.is_ascii_whitespace() && !matches!(b, b'/' | b'[' | b']' | b'<' | b'>' | b'(' | 0)
        }) {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    fn value(&mut self, depth: usize) -> Result<Engine> {
        ensure!(
            depth < MAX_ENGINE_DEPTH,
            "Photoshop text data is nested too deeply"
        );
        self.items += 1;
        ensure!(
            self.items <= MAX_ENGINE_ITEMS,
            "Photoshop text data is too large"
        );
        self.space();
        if self.starts(b"<<") {
            self.pos += 2;
            let mut items = Vec::new();
            loop {
                self.space();
                if self.starts(b">>") {
                    self.pos += 2;
                    return Ok(Engine::Dict(items));
                }
                ensure!(self.peek() == Some(b'/'), "Invalid Photoshop text data");
                self.pos += 1;
                let key = String::from_utf8_lossy(self.word()).into_owned();
                let value = self.value(depth + 1)?;
                items.push((key, value));
            }
        }
        match self.peek() {
            None => bail!("Invalid Photoshop text data"),
            Some(b'[') => {
                self.pos += 1;
                let mut values = Vec::new();
                loop {
                    self.space();
                    if self.peek() == Some(b']') {
                        self.pos += 1;
                        return Ok(Engine::Array(values));
                    }
                    values.push(self.value(depth + 1)?);
                }
            }
            Some(b'(') => {
                self.pos += 1;
                let mut bytes = Vec::new();
                loop {
                    let Some(b) = self.peek() else {
                        bail!("Invalid Photoshop text data")
                    };
                    self.pos += 1;
                    match b {
                        b'\\' => {
                            let Some(escaped) = self.peek() else {
                                bail!("Invalid Photoshop text data")
                            };
                            self.pos += 1;
                            bytes.push(match escaped {
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                other => other,
                            });
                        }
                        b')' => break,
                        _ => bytes.push(b),
                    }
                }
                Ok(Engine::String(decode_string(&bytes)))
            }
            Some(b'/') => {
                self.pos += 1;
                Ok(Engine::Name(
                    String::from_utf8_lossy(self.word()).into_owned(),
                ))
            }
            Some(_) => {
                let word = self.word();
                ensure!(!word.is_empty(), "Invalid Photoshop text data");
                match word {
                    b"true" => Ok(Engine::Bool(true)),
                    b"false" => Ok(Engine::Bool(false)),
                    _ => {
                        let text = std::str::from_utf8(word)
                            .map_err(|_| anyhow::anyhow!("Invalid Photoshop text data"))?;
                        let number: f64 = text
                            .parse()
                            .map_err(|_| anyhow::anyhow!("Invalid Photoshop text data"))?;
                        Ok(Engine::Number(number))
                    }
                }
            }
        }
    }
}

/// EngineData strings are UTF-16BE with a byte-order mark, or plain bytes.
fn decode_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        super::utf16(rest)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}
