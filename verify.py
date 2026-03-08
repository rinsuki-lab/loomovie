#!/usr/bin/env python3
from typing import IO
import zipfile
import json
import argparse
import os.path

def verify_stream(stream: IO[bytes], orig_path: str):
    BLOCK_SIZE = 1024 * 1024
    with open(orig_path, "rb") as f:
        while True:
            orig_block = f.read(BLOCK_SIZE)
            new_block = stream.read(BLOCK_SIZE)
            if orig_block != new_block:
                raise ValueError(f"Stream {orig_path} does not match original")
            if not orig_block:
                break


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("streams_json")
    parser.add_argument("mp4_file")
    args = parser.parse_args()

    with open(args.streams_json) as f:
        streams = json.load(f)
    with zipfile.ZipFile(args.mp4_file) as zf:
        for i, stream in enumerate(streams["streams"]):
            with zf.open(f"streams.{i}/init.m4s", "r") as f:
                verify_stream(f, os.path.join(os.path.dirname(args.streams_json), stream["init"]))
            for j, segment in enumerate(stream["chunks"]):
                with zf.open(f"streams.{i}/chunks/chunk.{j:06d}.m4s", "r") as f:
                    verify_stream(f, os.path.join(os.path.dirname(args.streams_json), os.path.dirname(stream["init"]), segment))
if __name__ == "__main__":
    main()
