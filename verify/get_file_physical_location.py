import sys
import struct
import fcntl
import os.path

if sys.platform == "linux":
    # https://www.kernel.org/doc/Documentation/filesystems/fiemap.txt
    FIEMAP_STRUCT = "=QQLLLL"
    FIEMAP_EXTENT_STRUCT = "QQQQQLLLL"
    FIEMAP_EXTENT_STRUCT_EMPTY = (0, 0, 0, 0, 0, 0, 0, 0, 0)
    def get_file_physical_location(path: str) -> int:
        with open(path, "rb") as f:
            fiemap = bytearray(struct.calcsize(FIEMAP_STRUCT) + struct.calcsize(FIEMAP_EXTENT_STRUCT))
            struct.pack_into(
                FIEMAP_STRUCT,
                fiemap,
                0,
                # --- struct fiemap
                0, # fm_start
                os.path.getsize(f.fileno()), # fm_length,
                0x0000_0001, # fm_flags (FIEMAP_FLAG_SYNC)
                0, # fm_mapped_extents
                1, # fm_extent_count
                0, # fm_reserved
            )
            struct.pack_into(FIEMAP_EXTENT_STRUCT, fiemap, struct.calcsize(FIEMAP_STRUCT), *FIEMAP_EXTENT_STRUCT_EMPTY)
            fcntl.ioctl(f.fileno(), 0xc020660b, fiemap) # FS_IOC_FIEMAP
            fm_mapped_extents = struct.unpack_from(FIEMAP_STRUCT, fiemap, 0)[3]
            assert fm_mapped_extents == 1
            extent = struct.unpack_from(FIEMAP_EXTENT_STRUCT, fiemap, struct.calcsize(FIEMAP_STRUCT))
            physical_offset = extent[1]
            return physical_offset
else:
    def get_file_physical_location(path: str) -> int:
        return 0