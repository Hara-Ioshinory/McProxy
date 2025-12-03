use std::io::Result;
use crate::consts::MAX_STRING_LEN;

pub struct VarInt;
impl VarInt {
    pub fn read_from_slice(buf: &mut &[u8]) -> Result<i32> {
        let mut num_read = 0;
        let mut result: i32 = 0;
        let mut shift = 0;
        loop {
            if buf.is_empty() {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "unexpected eof reading varint"));
            }
            let byte = buf[0];
            *buf = &buf[1..];
            let value = (byte & 0x7F) as i32;
            result |= value << shift;
            shift += 7;
            num_read += 1;
            if num_read > 5 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "varint too big"));
            }
            if (byte & 0x80) == 0 { break; }
        }
        Ok(result)
    }
}

pub fn read_varint_string_from_slice(buf: &mut &[u8]) -> Result<String> {
    let len = VarInt::read_from_slice(buf)?;
    if len < 0 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "negative length")); }
    let len = len as usize;
    if len > MAX_STRING_LEN { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "string too large")); }
    if buf.len() < len { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "unexpected eof reading string")); }
    let s = &buf[..len];
    let res = std::str::from_utf8(s).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8"))?.to_string();
    *buf = &buf[len..];
    Ok(res)
}
