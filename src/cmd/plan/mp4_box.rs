// MP4 box parsing and construction utilities

use bytes::{BufMut, BytesMut};

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
    let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as u64;
    let box_type: [u8; 4] = data[offset + 4..offset + 8].try_into().unwrap();
    let (header_size, total_size) = if size == 1 {
        if offset + 16 > data.len() {
            return None;
        }
        let ext = u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap());
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
    let flags = ((content[1] as u32) << 16) | ((content[2] as u32) << 8) | (content[3] as u32);
    (version, flags, &content[4..])
}

// ===== Box Writing =====

/// Guard returned by `start_box` / `start_fullbox`.
/// Panics on drop if `finish()` is not called.
/// Implements `DerefMut<Target = BytesMut>` so callers write directly through the guard.
#[must_use = "call .finish() to complete the box"]
pub struct BoxStart<'a> {
    buf: &'a mut BytesMut,
    start: usize,
}

impl std::ops::Deref for BoxStart<'_> {
    type Target = BytesMut;
    fn deref(&self) -> &BytesMut {
        self.buf
    }
}

impl std::ops::DerefMut for BoxStart<'_> {
    fn deref_mut(&mut self) -> &mut BytesMut {
        self.buf
    }
}

impl BoxStart<'_> {
    /// Patch the size field and complete the box.
    pub fn finish(self) {
        let size = (self.buf.len() - self.start) as u32;
        self.buf[self.start..self.start + 4].copy_from_slice(&size.to_be_bytes());
        std::mem::forget(self);
    }
}

impl Drop for BoxStart<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            panic!(
                "BoxStart dropped without calling finish() — box at offset {} was never completed",
                self.start
            );
        }
    }
}

/// Begin writing a box. Returns a guard that must be finished with `.finish()`.
pub fn start_box<'a>(buf: &'a mut BytesMut, box_type: &[u8; 4]) -> BoxStart<'a> {
    let start = buf.len();
    buf.put_u32(0); // placeholder for size
    buf.put_slice(box_type);
    BoxStart { buf, start }
}

/// Begin writing a full box (with version + flags). Returns a guard that must be finished.
pub fn start_fullbox<'a>(
    buf: &'a mut BytesMut,
    box_type: &[u8; 4],
    version: u8,
    flags: u32,
) -> BoxStart<'a> {
    let start = buf.len();
    buf.put_u32(0); // placeholder for size
    buf.put_slice(box_type);
    buf.put_u8(version);
    buf.put_u8(((flags >> 16) & 0xFF) as u8);
    buf.put_u8(((flags >> 8) & 0xFF) as u8);
    buf.put_u8((flags & 0xFF) as u8);
    BoxStart { buf, start }
}

pub fn make_free_header(content_size: usize) -> [u8; 8] {
    let size = (8 + content_size) as u32;
    let mut buf = [0u8; 8];
    buf[0..4].copy_from_slice(&size.to_be_bytes());
    buf[4..8].copy_from_slice(b"free");
    buf
}
