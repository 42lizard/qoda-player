// Bound decoder dimensions as well as transport size. QML handles actual decoding.
pub fn bounded_jpeg(bytes: &[u8]) -> bool {
    if bytes.len() > 256 * 1024
        || !bytes.starts_with(&[0xff, 0xd8])
        || !bytes.ends_with(&[0xff, 0xd9])
    {
        return false;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xff {
            return false;
        }
        while i < bytes.len() && bytes[i] == 0xff {
            i += 1;
        }
        let Some(&marker) = bytes.get(i) else {
            return false;
        };
        i += 1;
        if marker == 0xda || marker == 0xd9 {
            return false;
        }
        if i + 2 > bytes.len() {
            return false;
        }
        let size = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if size < 2 || i + size > bytes.len() {
            return false;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if size < 8 {
                return false;
            }
            let height = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]);
            let width = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
            return (1..=512).contains(&height) && (1..=512).contains(&width);
        }
        i += size;
    }
    false
}
