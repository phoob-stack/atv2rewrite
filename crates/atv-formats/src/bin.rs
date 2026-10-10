//! Little-endian cursor used by the binary format readers.

use crate::{Error, Result};

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, pos: 0, what }
    }
    pub fn at(data: &'a [u8], pos: usize, what: &'static str) -> Self {
        Self { data, pos, what }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof(self.what))?;
        let s = self.data.get(self.pos..end).ok_or(Error::Eof(self.what))?;
        self.pos = end;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn vec2(&mut self) -> Result<[f32; 2]> {
        Ok([self.f32()?, self.f32()?])
    }
    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    pub fn f32s<const N: usize>(&mut self) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f32()?;
        }
        Ok(out)
    }
    /// Reads a fixed-size, NUL-terminated name field.
    pub fn name(&mut self, len: usize) -> Result<String> {
        Ok(cstr(self.bytes(len)?))
    }
    /// Reads a NUL-terminated string of any length.
    pub fn cstr(&mut self) -> Result<String> {
        let rest = self.data.get(self.pos..).ok_or(Error::Eof(self.what))?;
        let len = rest.iter().position(|&c| c == 0).ok_or(Error::Eof(self.what))?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }
    /// Element count from the file, checked against the bytes left so a
    /// corrupt count fails cleanly instead of allocating gigabytes.
    pub fn count(&mut self, elem_size: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(elem_size.max(1)) > self.remaining() {
            return Err(Error::Format(format!("{}: count {n} exceeds data", self.what)));
        }
        Ok(n)
    }
}

pub fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}
