/// ZIP format helpers for generating PKZIP-compatible entries with ZIP64 extensions.

/// Information about a ZIP file entry, collected for central directory generation.
pub struct ZipFileEntry {
    pub filename: Vec<u8>,
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
    let mut buf = Vec::with_capacity(local_file_header_size(filename.len()));
    buf.extend_from_slice(&0x04034b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&45u16.to_le_bytes()); // version needed (4.5 for ZIP64)
    buf.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
    buf.extend_from_slice(&compression_method.to_le_bytes()); // compression method
    buf.extend_from_slice(&0u16.to_le_bytes()); // last mod file time
    buf.extend_from_slice(&0u16.to_le_bytes()); // last mod file date
    buf.extend_from_slice(&crc32.to_le_bytes()); // crc-32
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // compressed size (ZIP64)
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // uncompressed size (ZIP64)
    buf.extend_from_slice(&(filename.len() as u16).to_le_bytes()); // file name length
    buf.extend_from_slice(&20u16.to_le_bytes()); // extra field length
    buf.extend_from_slice(filename); // file name
    // ZIP64 extended information extra field
    buf.extend_from_slice(&0x0001u16.to_le_bytes()); // header id
    buf.extend_from_slice(&16u16.to_le_bytes()); // data size
    buf.extend_from_slice(&uncompressed_size.to_le_bytes()); // original uncompressed size
    buf.extend_from_slice(&compressed_size.to_le_bytes()); // compressed size
    buf
}

/// Generate a ZIP central directory file header entry.
fn make_cd_entry(entry: &ZipFileEntry) -> Vec<u8> {
    let mut buf = Vec::with_capacity(46 + entry.filename.len() + 28);
    buf.extend_from_slice(&0x02014b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&((3u16 << 8) | 45).to_le_bytes()); // version made by (UNIX, 4.5)
    buf.extend_from_slice(&45u16.to_le_bytes()); // version needed
    buf.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
    buf.extend_from_slice(&entry.compression_method.to_le_bytes()); // compression method
    buf.extend_from_slice(&0u16.to_le_bytes()); // last mod file time
    buf.extend_from_slice(&0u16.to_le_bytes()); // last mod file date
    buf.extend_from_slice(&entry.crc32.to_le_bytes()); // crc-32
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // compressed size (ZIP64)
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // uncompressed size (ZIP64)
    buf.extend_from_slice(&(entry.filename.len() as u16).to_le_bytes()); // file name length
    buf.extend_from_slice(&28u16.to_le_bytes()); // extra field length (ZIP64 with offset)
    buf.extend_from_slice(&0u16.to_le_bytes()); // file comment length
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk number start
    buf.extend_from_slice(&0u16.to_le_bytes()); // internal file attributes
    buf.extend_from_slice(&0u32.to_le_bytes()); // external file attributes
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // relative offset (ZIP64)
    buf.extend_from_slice(&entry.filename); // file name
    // ZIP64 extended information extra field
    buf.extend_from_slice(&0x0001u16.to_le_bytes()); // header id
    buf.extend_from_slice(&24u16.to_le_bytes()); // data size (8+8+8)
    buf.extend_from_slice(&entry.uncompressed_size.to_le_bytes()); // original uncompressed size
    buf.extend_from_slice(&entry.compressed_size.to_le_bytes()); // compressed size
    buf.extend_from_slice(&entry.local_header_offset.to_le_bytes()); // offset of local header
    buf
}

/// Generate ZIP end-of-archive records: central directory + ZIP64 EOCD + locator + EOCD.
pub fn make_end_records(entries: &[ZipFileEntry], cd_offset: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    // Central directory entries
    for entry in entries {
        buf.extend_from_slice(&make_cd_entry(entry));
    }
    let cd_size = buf.len() as u64;
    let zip64_eocd_offset = cd_offset + cd_size;

    // ZIP64 End of Central Directory Record
    buf.extend_from_slice(&0x06064b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&44u64.to_le_bytes()); // size of remaining record
    buf.extend_from_slice(&((3u16 << 8) | 45).to_le_bytes()); // version made by
    buf.extend_from_slice(&45u16.to_le_bytes()); // version needed
    buf.extend_from_slice(&0u32.to_le_bytes()); // disk number
    buf.extend_from_slice(&0u32.to_le_bytes()); // disk with CD start
    buf.extend_from_slice(&(entries.len() as u64).to_le_bytes()); // entries on this disk
    buf.extend_from_slice(&(entries.len() as u64).to_le_bytes()); // total entries
    buf.extend_from_slice(&cd_size.to_le_bytes()); // size of central directory
    buf.extend_from_slice(&cd_offset.to_le_bytes()); // offset of central directory

    // ZIP64 End of Central Directory Locator
    buf.extend_from_slice(&0x07064b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&0u32.to_le_bytes()); // disk with ZIP64 EOCD
    buf.extend_from_slice(&zip64_eocd_offset.to_le_bytes()); // offset of ZIP64 EOCD
    buf.extend_from_slice(&1u32.to_le_bytes()); // total disks

    // End of Central Directory Record
    let entries_count = entries.len() as u64;
    let entries_16 = if entries_count > u16::MAX as u64 { u16::MAX } else { entries_count as u16 };
    let cd_size_32 = if cd_size > u32::MAX as u64 { u32::MAX } else { cd_size as u32 };
    let cd_offset_32 = if cd_offset > u32::MAX as u64 { u32::MAX } else { cd_offset as u32 };
    buf.extend_from_slice(&0x06054b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk number
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk with CD start
    buf.extend_from_slice(&entries_16.to_le_bytes()); // entries on this disk
    buf.extend_from_slice(&entries_16.to_le_bytes()); // total entries
    buf.extend_from_slice(&cd_size_32.to_le_bytes()); // size of central directory
    buf.extend_from_slice(&cd_offset_32.to_le_bytes()); // offset of central directory
    buf.extend_from_slice(&0u16.to_le_bytes()); // comment length
    buf
}
