#!/usr/bin/env python3
"""Bound launchd stdout/stderr logs without reading an entire oversized file.

Run once per minute as the Hagency service user. Keep the last 1 MiB of a log
larger than 20 MiB, then truncate its existing inode so open writers keep working.
"""
import json
import os
from pathlib import Path
import stat
import sys
import tempfile

MAX_LOG_BYTES = 20 * 1024 * 1024
TAIL_BYTES = 1024 * 1024
LOG_NAMES = ("server.log", "server-error.log")


def rotate(directory):
    for name in LOG_NAMES:
        path = directory / name
        try:
            fd = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK)
        except FileNotFoundError:
            continue
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid():
                raise ValueError("Log must be a regular file owned by the service user")
            if info.st_size <= MAX_LOG_BYTES:
                continue
            # pread's count is bounded even when the log is hundreds of GB.
            recent = os.pread(fd, TAIL_BYTES, max(0, info.st_size - TAIL_BYTES))
            tmp_fd, tmp_path = tempfile.mkstemp(prefix="." + name + "-", dir=directory)
            try:
                with os.fdopen(tmp_fd, "wb") as output:
                    output.write(recent)
                    output.flush()
                    os.fsync(output.fileno())
                os.replace(tmp_path, directory / (name + ".previous-tail"))
                os.ftruncate(fd, 0)
            finally:
                if os.path.exists(tmp_path):
                    os.unlink(tmp_path)
            print(json.dumps({"rotated": name, "previous_bytes": info.st_size,
                              "preserved_bytes": len(recent)}), flush=True)
        finally:
            os.close(fd)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: rotate-service-logs.py LOG_DIRECTORY")
    rotate(Path(sys.argv[1]))
