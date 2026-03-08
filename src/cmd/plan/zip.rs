/// ZIP format helpers for generating PKZIP-compatible entries with ZIP64 extensions.

use bytes::{BufMut, Bytes, BytesMut};

/// Information about a ZIP file entry, collected for central directory generation.
pub struct ZipFileEntry {
    pub filename: Bytes,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub compression_method: u16,
    pub local_header_offset: u64,
}

pub fn entry_name_init(stream_idx: usize) -> String {
    format!("streams.{}/init.m4s", stream_idx)
}

pub fn entry_name_chunk(stream_idx: usize, chunk_idx: usize) -> String {
    format!("streams.{}/chunks/chunk.{:06}.m4s", stream_idx, chunk_idx)
}

/// Size of a ZIP local file header with ZIP64 extra field.
pub fn local_file_header_size(filename_len: usize) -> usize {
    30 + filename_len + 20 // 20 = ZIP64 extra field (4 byte header + 16 byte data)
}

/// Generate a ZIP local file header with ZIP64 extensions.
pub fn make_local_file_header(
    filename: &[u8],
    crc32: u32,
    compression_method: u16,
    compressed_size: u64,
    uncompressed_size: u64,
) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(local_file_header_size(filename.len()));
    buf.put_u32_le(0x04034b50); // signature
    buf.put_u16_le(45); // version needed (4.5 for ZIP64)
    buf.put_u16_le(0); // general purpose bit flag
    buf.put_u16_le(compression_method); // compression method
    buf.put_u16_le(0); // last mod file time
    buf.put_u16_le(0); // last mod file date
    buf.put_u32_le(crc32); // crc-32
    buf.put_u32_le(0xFFFFFFFF); // compressed size (ZIP64)
    buf.put_u32_le(0xFFFFFFFF); // uncompressed size (ZIP64)
    buf.put_u16_le(filename.len() as u16); // file name length
    buf.put_u16_le(20); // extra field length
    buf.put_slice(filename); // file name
    // ZIP64 extended information extra field
    buf.put_u16_le(0x0001); // header id
    buf.put_u16_le(16); // data size
    buf.put_u64_le(uncompressed_size); // original uncompressed size
    buf.put_u64_le(compressed_size); // compressed size
    buf.into()
}

/// Generate a ZIP central directory file header entry.
fn make_cd_entry(entry: &ZipFileEntry) -> Vec<u8> {
    let mut buf = BytesMut::with_capacity(46 + entry.filename.len() + 28);
    buf.put_u32_le(0x02014b50); // signature
    buf.put_u16_le((3u16 << 8) | 45); // version made by (UNIX, 4.5)
    buf.put_u16_le(45); // version needed
    buf.put_u16_le(0); // general purpose bit flag
    buf.put_u16_le(entry.compression_method); // compression method
    buf.put_u16_le(0); // last mod file time
    buf.put_u16_le(0); // last mod file date
    buf.put_u32_le(entry.crc32); // crc-32
    buf.put_u32_le(0xFFFFFFFF); // compressed size (ZIP64)
    buf.put_u32_le(0xFFFFFFFF); // uncompressed size (ZIP64)
    buf.put_u16_le(entry.filename.len() as u16); // file name length
    buf.put_u16_le(28); // extra field length (ZIP64 with offset)
    buf.put_u16_le(0); // file comment length
    buf.put_u16_le(0); // disk number start
    buf.put_u16_le(0); // internal file attributes
    buf.put_u32_le(0); // external file attributes
    buf.put_u32_le(0xFFFFFFFF); // relative offset (ZIP64)
    buf.put_slice(&entry.filename); // file name
    // ZIP64 extended information extra field
    buf.put_u16_le(0x0001); // header id
    buf.put_u16_le(24); // data size (8+8+8)
    buf.put_u64_le(entry.uncompressed_size); // original uncompressed size
    buf.put_u64_le(entry.compressed_size); // compressed size
    buf.put_u64_le(entry.local_header_offset); // offset of local header
    buf.into()
}

/// Generate ZIP end-of-archive records: central directory + ZIP64 EOCD + locator + EOCD.
pub fn make_end_records(entries: &[ZipFileEntry], cd_offset: u64) -> Vec<u8> {
    // CD entry = 46 + name.len() + 28; fixed end records = 56 + 20 + 22 = 98
    let cd_est: usize = entries.iter().map(|e| 74 + e.filename.len()).sum();
    let mut buf = BytesMut::with_capacity(cd_est + 98);
    // Central directory entries
    for entry in entries {
        buf.put_slice(&make_cd_entry(entry));
    }
    let cd_size = buf.len() as u64;
    let zip64_eocd_offset = cd_offset + cd_size;

    // ZIP64 End of Central Directory Record
    buf.put_u32_le(0x06064b50); // signature
    buf.put_u64_le(44); // size of remaining record
    buf.put_u16_le((3u16 << 8) | 45); // version made by
    buf.put_u16_le(45); // version needed
    buf.put_u32_le(0); // disk number
    buf.put_u32_le(0); // disk with CD start
    buf.put_u64_le(entries.len() as u64); // entries on this disk
    buf.put_u64_le(entries.len() as u64); // total entries
    buf.put_u64_le(cd_size); // size of central directory
    buf.put_u64_le(cd_offset); // offset of central directory

    // ZIP64 End of Central Directory Locator
    buf.put_u32_le(0x07064b50); // signature
    buf.put_u32_le(0); // disk with ZIP64 EOCD
    buf.put_u64_le(zip64_eocd_offset); // offset of ZIP64 EOCD
    buf.put_u32_le(1); // total disks

    // End of Central Directory Record
    let entries_count = entries.len() as u64;
    let entries_16 = if entries_count > u16::MAX as u64 { u16::MAX } else { entries_count as u16 };
    let cd_size_32 = if cd_size > u32::MAX as u64 { u32::MAX } else { cd_size as u32 };
    let cd_offset_32 = if cd_offset > u32::MAX as u64 { u32::MAX } else { cd_offset as u32 };
    buf.put_u32_le(0x06054b50); // signature
    buf.put_u16_le(0); // disk number
    buf.put_u16_le(0); // disk with CD start
    buf.put_u16_le(entries_16); // entries on this disk
    buf.put_u16_le(entries_16); // total entries
    buf.put_u32_le(cd_size_32); // size of central directory
    buf.put_u32_le(cd_offset_32); // offset of central directory
    buf.put_u16_le(0); // comment length
    buf.into()
}
