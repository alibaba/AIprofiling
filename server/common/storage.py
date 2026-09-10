# SPDX-License-Identifier: Apache-2.0
"""Blob store abstraction — filesystem by default, MinIO/S3 possible.

The reference implementation writes to a directory; callers get and put
by relative "key". Keeping the interface small so future S3 backends
only need to implement `put_bytes`, `get_bytes`, `open_stream`.
"""

from __future__ import annotations

import io
import os
import tarfile
from pathlib import Path
from typing import Optional


class BlobStore:
    def __init__(self, base_dir: str) -> None:
        self.base = Path(base_dir).absolute()
        self.base.mkdir(parents=True, exist_ok=True)

    def put_bytes(self, key: str, data: bytes) -> str:
        path = self._resolve(key)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return str(path.relative_to(self.base))

    def get_bytes(self, key: str) -> Optional[bytes]:
        path = self._resolve(key)
        if not path.exists():
            return None
        return path.read_bytes()

    def extract_member(self, key: str, member_name: str) -> Optional[bytes]:
        """Pull a single named file from a tarball blob without full extract.

        `tarfile` auto-detects gz/bz2/xz but not zstd. The Python agent writes
        .tar.gz; the Rust agent writes .tar.zst. For zstd we decompress into an
        in-memory buffer first (sessions are small), then open as a plain tar.
        """
        path = self._resolve(key)
        if not path.exists():
            return None

        if str(path).endswith(".zst"):
            import zstandard

            dctx = zstandard.ZstdDecompressor()
            with path.open("rb") as fh:
                raw = dctx.stream_reader(fh).read()
            tar_cm = tarfile.open(fileobj=io.BytesIO(raw), mode="r:")
        else:
            tar_cm = tarfile.open(path, mode="r:*")

        with tar_cm as tar:
            for m in tar.getmembers():
                # Match both "<sid>/<name>" and bare "<name>".
                if m.name == member_name or m.name.endswith("/" + member_name):
                    f = tar.extractfile(m)
                    if f is None:
                        return None
                    return f.read()
        return None

    def _resolve(self, key: str) -> Path:
        key = key.lstrip("/")
        p = (self.base / key).resolve()
        if not str(p).startswith(str(self.base)):
            raise ValueError("blob key escapes base directory")
        return p
