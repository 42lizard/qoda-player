//! Qualified iTunes freeform hierarchy; opaque media/artwork atoms are skipped.
use super::{LIMIT, Reader, SCALAR, SPAN, Tags, candidate};
use crate::gain::{Fields, TagSource};
const INVALID: &str = "Invalid embedded MP4 metadata";
const ATOMS: usize = 1024;
struct Atom {
    kind: [u8; 4],
    data: usize,
    end: usize,
}
async fn atom(
    reader: &mut Reader<'_>,
    pos: usize,
    parent: usize,
    count: &mut usize,
) -> Result<Atom, &'static str> {
    *count += 1;
    if *count > ATOMS {
        return Err(LIMIT);
    }
    if parent.saturating_sub(pos) < 8 {
        return Err(INVALID);
    }
    let header = reader.get(pos, 8).await?;
    let short = u32::from_be_bytes(header[..4].try_into().unwrap());
    let (size, head) = match short {
        0 => (reader.total.checked_sub(pos).ok_or(INVALID)?, 8),
        1 => {
            if parent - pos < 16 {
                return Err(INVALID);
            }
            let wide = reader.get(pos + 8, 8).await?;
            (
                usize::try_from(u64::from_be_bytes(wide.try_into().unwrap()))
                    .map_err(|_| INVALID)?,
                16,
            )
        }
        n => (n as usize, 8),
    };
    let end = pos
        .checked_add(size)
        .filter(|end| *end <= parent)
        .ok_or(INVALID)?;
    if size < head {
        return Err(INVALID);
    }
    Ok(Atom {
        kind: header[4..8].try_into().unwrap(),
        data: pos + head,
        end,
    })
}
async fn label(reader: &mut Reader<'_>, value: &Atom) -> Result<String, &'static str> {
    let size = value.end - value.data;
    if size < 4 {
        return Err(INVALID);
    }
    if size > 1024 {
        return Err(LIMIT);
    }
    let bytes = reader.get(value.data, size).await?;
    if bytes[..4] != [0; 4] {
        return Err(INVALID);
    }
    std::str::from_utf8(&bytes[4..])
        .map(str::to_owned)
        .map_err(|_| INVALID)
}
async fn freeform(
    reader: &mut Reader<'_>,
    item: &Atom,
    count: &mut usize,
    fields: &mut Fields,
) -> Result<(), &'static str> {
    let mut mean = None;
    let mut name = None;
    let mut values = Vec::new();
    let mut pos = item.data;
    while pos < item.end {
        let child = atom(reader, pos, item.end, count).await?;
        pos = child.end;
        match &child.kind {
            b"mean" => {
                if mean.is_some() {
                    return Err(INVALID);
                }
                mean = Some(label(reader, &child).await?);
            }
            b"name" => {
                if name.is_some() {
                    return Err(INVALID);
                }
                name = Some(label(reader, &child).await?);
            }
            b"data" => values.push(child),
            _ => {}
        }
    }
    let mean = mean.ok_or(INVALID)?;
    let name = name.ok_or(INVALID)?;
    if mean != "com.apple.iTunes" {
        return Ok(());
    }
    if Fields::index(&name).is_none() {
        return Ok(());
    }
    if values.is_empty() {
        fields.push(&name, None);
    }
    for value in values {
        let length = value.end - value.data;
        if !(8..=SCALAR).contains(&length) {
            fields.push(&name, None);
            continue;
        }
        let bytes = reader.get(value.data, length).await?;
        let text = if bytes[..4] == [0, 0, 0, 1] {
            std::str::from_utf8(&bytes[8..]).ok()
        } else {
            None
        };
        fields.push(&name, text);
    }
    Ok(())
}
async fn ilst(
    reader: &mut Reader<'_>,
    list: Atom,
    count: &mut usize,
) -> Result<Tags, &'static str> {
    let mut fields = Fields::default();
    let mut pos = list.data;
    while pos < list.end {
        let item = atom(reader, pos, list.end, count).await?;
        pos = item.end;
        if &item.kind == b"----" {
            freeform(reader, &item, count, &mut fields).await?;
        }
    }
    Ok(fields.finish(TagSource::Mp4))
}
async fn meta(
    reader: &mut Reader<'_>,
    container: Atom,
    count: &mut usize,
) -> Result<Tags, &'static str> {
    if container.end - container.data < 4 || reader.get(container.data, 4).await? != [0; 4] {
        return Err("MP4 metadata layout is not qualified; unity gain");
    }
    let mut pos = container.data + 4;
    let mut handler = None;
    let mut list = None;
    while pos < container.end {
        let child = atom(reader, pos, container.end, count).await?;
        pos = child.end;
        match &child.kind {
            b"hdlr" => {
                if handler.is_some() || child.end - child.data < 12 {
                    return Err(INVALID);
                }
                let header = reader.get(child.data, 12).await?;
                if header[..4] != [0; 4] {
                    return Err(INVALID);
                }
                handler = Some(header[8..12].to_vec());
            }
            b"ilst" => {
                if list.is_some() {
                    return Err(INVALID);
                }
                list = Some(child);
            }
            _ => {}
        }
    }
    if handler.as_deref().is_some_and(|handler| handler != b"mdir") {
        return Err("MP4 metadata handler is not qualified; unity gain");
    }
    match list {
        Some(list) => ilst(reader, list, count).await,
        None => Ok(Tags::default()),
    }
}
#[derive(Clone, Copy)]
enum Level {
    Root,
    Movie,
    User,
}
pub(super) async fn scan(reader: &mut Reader<'_>) -> Result<Tags, &'static str> {
    let mut pending = vec![(0, reader.total, Level::Root)];
    let mut count = 0;
    let mut found = None;
    while let Some((mut pos, end, level)) = pending.pop() {
        while pos < end {
            let item = atom(reader, pos, end, &mut count).await?;
            pos = item.end;
            match (level, &item.kind) {
                (Level::Root, b"ftyp") => {
                    if item.end - item.data < 8 || (item.end - item.data) % 4 != 0 {
                        return Err(INVALID);
                    }
                }
                (Level::Root, b"moov") => {
                    if item.end - item.data > SPAN {
                        return Err(LIMIT);
                    }
                    pending.push((item.data, item.end, Level::Movie));
                }
                (Level::Movie, b"udta") => pending.push((item.data, item.end, Level::User)),
                (Level::Movie | Level::User, b"meta") => {
                    candidate(&mut found, meta(reader, item, &mut count).await?)?
                }
                _ => {}
            }
        }
    }
    Ok(found.unwrap_or_default())
}
