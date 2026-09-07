//! Variable Length Quantity, RP-001 §"Variable Length Quantity".
//!
//! Seven bits per byte, most significant first, with bit 7 set on every byte
//! but the last. The spec caps a quantity at four bytes (0x0FFFFFFF).

/// Reads a VLQ at `*pos`, advancing it past the quantity. `None` if the buffer
/// ends first or the quantity runs longer than the four bytes the spec allows.
#[inline]
pub fn read(buf: &[u8], pos: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..4 {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value = (value << 7) | u32::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Appends `value` as a VLQ. Values above 0x0FFFFFFF are not representable and
/// are clamped by the caller before they get here.
pub fn write(out: &mut Vec<u8>, value: u32) {
    let mut buffer = [0u8; 4];
    let mut len = 1;
    buffer[3] = (value & 0x7F) as u8;

    let mut rest = value >> 7;
    while rest != 0 {
        buffer[4 - len - 1] = (rest & 0x7F) as u8 | 0x80;
        rest >>= 7;
        len += 1;
    }

    out.extend_from_slice(&buffer[4 - len..]);
}
