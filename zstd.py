"""Compatibility API backed by the Windows-wheel-supported zstandard package."""
import zstandard

Error = zstandard.ZstdError

def compress(data, level=3):
    return zstandard.ZstdCompressor(level=level).compress(data)

def decompress(data):
    return zstandard.ZstdDecompressor().decompress(data)
