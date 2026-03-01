// MP4 box parsing and construction utilities

use crate::binary::*;

#[derive(Debug, Clone)]
pub struct BoxInfo {
    pub offset: usize,
    pub header_size: usize,
    pub total_size: usize,
    pub box_type: [u8; 4],
}

pub fn parse_box_at(data: &[u8], offset: usize) -> Option<BoxInfo> {
    if offset + 8 > data.len() {
        return None;
    }
    let size = read_u32_be(data, offset) as u64;
    let box_type: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
    let (header_size, total_size) = if size == 1 {
        if offset + 16 > data.len() {
            return None;
        }
        let ext = read_u64_be(data, offset + 8);
        (16, ext as usize)
    } else if size == 0 {
        (8, data.len() - offset)
    } else {
        (8, size as usize)
    };
    Some(BoxInfo {
        offset,
        header_size,
        total_size,
        box_type,
    })
}

pub fn iter_boxes(data: &[u8]) -> Vec<BoxInfo> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset + 8 <= data.len() {
        if let Some(info) = parse_box_at(data, offset) {
            if info.total_size == 0 {
                break;
            }
            offset += info.total_size;
            result.push(info);
        } else {
            break;
        }
    }
    result
}

pub fn find_box(data: &[u8], box_type: &[u8; 4]) -> Option<BoxInfo> {
    iter_boxes(data)
        .into_iter()
        .find(|info| info.box_type == *box_type)
}

pub fn box_content<'a>(data: &'a [u8], info: &BoxInfo) -> &'a [u8] {
    &data[info.offset + info.header_size..info.offset + info.total_size]
}

pub fn box_raw<'a>(data: &'a [u8], info: &BoxInfo) -> &'a [u8] {
    &data[info.offset..info.offset + info.total_size]
}

pub fn fullbox_parse(content: &[u8]) -> (u8, u32, &[u8]) {
    let version = content[0];
    let flags =
        ((content[1] as u32) << 16) | ((content[2] as u32) << 8) | (content[3] as u32);
    (version, flags, &content[4..])
}

// ===== Box Writing =====

pub fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let size = (8 + content.len()) as u32;
    let mut buf = Vec::with_capacity(size as usize);
    write_u32_be(&mut buf, size);
    buf.extend_from_slice(box_type);
    buf.extend_from_slice(content);
    buf
}

pub fn make_fullbox(box_type: &[u8; 4], version: u8, flags: u32, content: &[u8]) -> Vec<u8> {
    let size = (12 + content.len()) as u32;
    let mut buf = Vec::with_capacity(size as usize);
    write_u32_be(&mut buf, size);
    buf.extend_from_slice(box_type);
    buf.push(version);
    buf.push(((flags >> 16) & 0xFF) as u8);
    buf.push(((flags >> 8) & 0xFF) as u8);
    buf.push((flags & 0xFF) as u8);
    buf.extend_from_slice(content);
    buf
}

pub fn make_free_header(content_size: usize) -> [u8; 8] {
    let size = (8 + content_size) as u32;
    let mut buf = [0u8; 8];
    buf[0..4].copy_from_slice(&size.to_be_bytes());
    buf[4..8].copy_from_slice(b"free");
    buf
}
