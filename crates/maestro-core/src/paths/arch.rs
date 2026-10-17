use std::path::Path;

pub fn file_arch(path: &Path) -> Option<&'static str> {
    let head = read_head(path)?;
    match head.get(..4)? {
        [0x7F, b'E', b'L', b'F'] => elf(&head),
        [b'M', b'Z', ..] => pe(&head),
        [0xFE, 0xED, 0xFA, 0xCE | 0xCF] => macho(u32::from_be_bytes(word(&head, 4)?)),
        [0xCE | 0xCF, 0xFA, 0xED, 0xFE] => macho(u32::from_le_bytes(word(&head, 4)?)),
        [0xCA, 0xFE, 0xBA, 0xBE | 0xBF] => fat(&head),
        _ => None,
    }
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0u8; 4096];
    let mut file = std::fs::File::open(path).ok()?;
    let mut len = 0;
    while len < buf.len() {
        match file.read(&mut buf[len..]).ok()? {
            0 => break,
            n => len += n,
        }
    }
    buf.truncate(len);
    Some(buf)
}

fn word(buf: &[u8], at: usize) -> Option<[u8; 4]> {
    buf.get(at..at + 4)?.try_into().ok()
}

fn half(buf: &[u8], at: usize, big_endian: bool) -> Option<u16> {
    let b: [u8; 2] = buf.get(at..at + 2)?.try_into().ok()?;
    Some(if big_endian {
        u16::from_be_bytes(b)
    } else {
        u16::from_le_bytes(b)
    })
}

fn elf(head: &[u8]) -> Option<&'static str> {
    match half(head, 0x12, *head.get(5)? == 2)? {
        0x03 => Some("x86"),
        0x28 => Some("arm"),
        0x3E => Some("x86_64"),
        0xB7 => Some("aarch64"),
        _ => None,
    }
}

fn pe(head: &[u8]) -> Option<&'static str> {
    let at = u32::from_le_bytes(word(head, 0x3C)?) as usize;
    if head.get(at..at + 4)? != b"PE\0\0" {
        return None;
    }
    match half(head, at + 4, false)? {
        0x014C => Some("x86"),
        0x01C4 => Some("arm"),
        0x8664 => Some("x86_64"),
        0xA641 => Some("arm64ec"),
        0xAA64 => Some("aarch64"),
        _ => None,
    }
}

fn macho(cputype: u32) -> Option<&'static str> {
    match cputype {
        0x0000_0007 => Some("x86"),
        0x0000_000C => Some("arm"),
        0x0100_0007 => Some("x86_64"),
        0x0100_000C => Some("aarch64"),
        _ => None,
    }
}

fn fat(head: &[u8]) -> Option<&'static str> {
    let stride = if head[3] == 0xBF { 32 } else { 20 };
    let count = u32::from_be_bytes(word(head, 4)?) as usize;
    let slices = (0..count).filter_map(|i| macho(u32::from_be_bytes(word(head, 8 + i * stride)?)));
    let mut first = None;
    for arch in slices {
        if arch == std::env::consts::ARCH {
            return Some(arch);
        }
        first.get_or_insert(arch);
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file_with(name: &str, patches: &[(usize, &[u8])]) -> PathBuf {
        let mut buf = vec![0u8; 0x100];
        for (at, bytes) in patches {
            buf[*at..*at + bytes.len()].copy_from_slice(bytes);
        }
        let path = std::env::temp_dir().join(format!("maestro-arch-{}-{name}", std::process::id()));
        std::fs::write(&path, buf).unwrap();
        path
    }

    #[test]
    fn reads_elf() {
        let elf = |m: u16| {
            file_with(
                &format!("elf{m}"),
                &[(0, b"\x7FELF\x02\x01"), (0x12, &m.to_le_bytes())],
            )
        };
        assert_eq!(file_arch(&elf(0x3E)), Some("x86_64"));
        assert_eq!(file_arch(&elf(0xB7)), Some("aarch64"));
    }

    #[test]
    fn reads_pe() {
        let pe = |m: u16| {
            file_with(
                &format!("pe{m}"),
                &[
                    (0, b"MZ"),
                    (0x3C, &[0x40]),
                    (0x40, b"PE\0\0"),
                    (0x44, &m.to_le_bytes()),
                ],
            )
        };
        assert_eq!(file_arch(&pe(0x8664)), Some("x86_64"));
        assert_eq!(file_arch(&pe(0xAA64)), Some("aarch64"));
    }

    #[test]
    fn reads_macho() {
        let thin = file_with(
            "macho",
            &[
                (0, &[0xCF, 0xFA, 0xED, 0xFE]),
                (4, &0x0100_0007u32.to_le_bytes()),
            ],
        );
        assert_eq!(file_arch(&thin), Some("x86_64"));
        let fat = file_with(
            "fat",
            &[
                (0, &[0xCA, 0xFE, 0xBA, 0xBE]),
                (4, &1u32.to_be_bytes()),
                (8, &0x0100_000Cu32.to_be_bytes()),
            ],
        );
        assert_eq!(file_arch(&fat), Some("aarch64"));
    }

    #[test]
    fn ignores_other_files() {
        assert_eq!(file_arch(&file_with("txt", &[(0, b"BASS license")])), None);
    }
}
