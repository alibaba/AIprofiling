# SPDX-License-Identifier: Apache-2.0
"""Unit tests for BlobStore.extract_member — the gz + zst code paths."""

from __future__ import annotations

import io
import os
import sys
import tarfile
from pathlib import Path

# Make `server.common.storage` importable without installing the package.
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from server.common.storage import BlobStore  # noqa: E402


def _make_tar_gz(tmp: Path, sid: str, members: dict) -> str:
    key = f"{sid}.tar.gz"
    out = tmp / key
    with tarfile.open(out, mode="w:gz") as tar:
        for name, data in members.items():
            info = tarfile.TarInfo(name=f"{sid}/{name}")
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return key


def _make_tar_zst(tmp: Path, sid: str, members: dict) -> str:
    import zstandard  # optional dep; installed in the repo's Python env

    plain = io.BytesIO()
    with tarfile.open(fileobj=plain, mode="w") as tar:
        for name, data in members.items():
            info = tarfile.TarInfo(name=f"{sid}/{name}")
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    key = f"{sid}.tar.zst"
    out = tmp / key
    out.write_bytes(zstandard.ZstdCompressor().compress(plain.getvalue()))
    return key


def test_extract_member_from_gzip(tmp_path):
    store = BlobStore(str(tmp_path))
    key = _make_tar_gz(tmp_path, "s1", {
        "folded.txt": b"main;work 42\nmain;idle 5\n",
        "meta.json": b'{"schema_version":1}',
    })
    got = store.extract_member(key, "folded.txt")
    assert got == b"main;work 42\nmain;idle 5\n"


def test_extract_member_from_zstd(tmp_path):
    store = BlobStore(str(tmp_path))
    key = _make_tar_zst(tmp_path, "s2", {
        "folded.txt": b"a;b;c 1\n",
        "meta.json": b'{}',
    })
    got = store.extract_member(key, "folded.txt")
    assert got == b"a;b;c 1\n"


def test_extract_missing_member_returns_none(tmp_path):
    store = BlobStore(str(tmp_path))
    key = _make_tar_gz(tmp_path, "s3", {"folded.txt": b"x 1\n"})
    assert store.extract_member(key, "does-not-exist.txt") is None


def test_extract_missing_blob_returns_none(tmp_path):
    store = BlobStore(str(tmp_path))
    assert store.extract_member("nope.tar.gz", "folded.txt") is None


def test_key_traversal_rejected(tmp_path):
    store = BlobStore(str(tmp_path))
    import pytest
    with pytest.raises(ValueError):
        store.extract_member("../../etc/passwd", "folded.txt")
