"""Content hashing for runtime components.

Hashes *file contents* (sha256), never metadata such as mtime or
permissions.  Symlinks are resolved to their ultimate target before
hashing; the raw link text (``os.readlink``) is reported separately so a
retargeted link is visible even when the resolved content is unchanged.

Stdlib only.
"""

from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional, Union

PathLike = Union[str, os.PathLike]

_CHUNK = 1024 * 1024  # 1 MiB streaming reads


def sha256_bytes(data: bytes) -> str:
    """Hex sha256 of a byte string."""
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: PathLike) -> str:
    """Hex sha256 of file contents.

    ``open()`` follows the full symlink chain, so the digest is always of
    the *resolved* content.  Raises OSError if the (resolved) file cannot
    be read — callers that tolerate missing files should use
    :func:`fingerprint` instead.
    """
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(_CHUNK), b""):
            h.update(chunk)
    return h.hexdigest()


@dataclass(frozen=True)
class FileFingerprint:
    """Content identity + symlink metadata for one path.

    ``link_target`` is the *literal* readlink text of the first link in
    the chain (``None`` for regular files) — deliberately the written
    link text rather than the absolute resolved path so manifests stay
    reproducible across checkout locations.
    """

    path: str
    exists: bool
    sha256: Optional[str]
    size: Optional[int]
    is_symlink: bool
    link_target: Optional[str]
    resolved_path: Optional[str]


def fingerprint(path: PathLike) -> FileFingerprint:
    """Fingerprint a path, resolving symlinks for content.

    - regular file: sha256 of contents, ``link_target=None``
    - symlink: sha256 of the resolved target's contents, plus the raw
      link text so a retarget is detectable even with identical content
    - missing / broken symlink: ``exists=False``, ``sha256=None``
    """
    p = Path(path)
    is_link = p.is_symlink()
    link_target = os.readlink(p) if is_link else None
    try:
        resolved = str(p.resolve(strict=True))
    except (FileNotFoundError, RuntimeError):
        # FileNotFoundError: broken link / missing file.
        # RuntimeError: symlink loop.
        resolved = None
    if resolved is None:
        return FileFingerprint(
            path=str(p),
            exists=False,
            sha256=None,
            size=None,
            is_symlink=is_link,
            link_target=link_target,
            resolved_path=None,
        )
    return FileFingerprint(
        path=str(p),
        exists=True,
        sha256=sha256_file(resolved),
        size=os.path.getsize(resolved),
        is_symlink=is_link,
        link_target=link_target,
        resolved_path=resolved,
    )


def sha256_text_file(path: PathLike) -> Optional[str]:
    """Raw-byte sha256 of a text file, or None if unreadable/missing."""
    fp = fingerprint(path)
    return fp.sha256
