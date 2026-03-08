#!/usr/bin/env python3
import zipfile
import json
import argparse
import os.path
import binascii

def verify_stream(zf: zipfile.ZipFile, zip_path: str, orig_path: str):
    info = zf.getinfo(zip_path)
    BLOCK_SIZE = 1024 * 1024
    crc = 0
    with zf.open(info) as stream:
        with open(orig_path, "rb") as f:
            while True:
                orig_block = f.read(BLOCK_SIZE)
                zf_block = stream.read(BLOCK_SIZE)
                crc = binascii.crc32(orig_block, crc)
                if orig_block != zf_block:
                    raise ValueError(f"Stream {zip_path} does not match original file {orig_path}")
                if not orig_block:
                    break
    if crc != info.CRC:
        raise ValueError(f"CRC mismatch for {zip_path}: expected {info.CRC:08x}, got {crc:08x}")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("streams_json")
    parser.add_argument("mp4_file")
    args = parser.parse_args()

    with open(args.streams_json) as f:
        streams = json.load(f)
    with zipfile.ZipFile(args.mp4_file) as zf:
        for i, stream in enumerate(streams["streams"]):
            verify_stream(zf, f"streams.{i}/init.m4s", os.path.join(os.path.dirname(args.streams_json), stream["init"]))
            for j, segment in enumerate(stream["chunks"]):
                verify_stream(zf, f"streams.{i}/chunks/chunk.{j:06d}.m4s", os.path.join(os.path.dirname(args.streams_json), os.path.dirname(stream["init"]), segment))
if __name__ == "__main__":
    main()
