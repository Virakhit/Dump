"""Generate the bundled app mark without image dependencies."""
from pathlib import Path
import struct
import zlib

size = 64
pixels = []
for y in range(size):
    row = bytearray()
    for x in range(size):
        white = ((x - 29) ** 2 + (y - 37) ** 2 <= 15 ** 2 or (39 <= x <= 46 and 12 <= y <= 50))
        hole = (x - 29) ** 2 + (y - 37) ** 2 <= 7 ** 2
        row.extend((255, 255, 255, 255) if white and not hole else (102, 84, 217, 255))
    pixels.append(b'\0' + row)

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(pixels))) + chunk(b'IEND', b'')
directory = Path(__file__).resolve().parents[1] / 'src-tauri' / 'icons'
directory.mkdir(exist_ok=True)
(directory / 'icon.png').write_bytes(png)
(directory / 'icon.ico').write_bytes(struct.pack('<HHH', 0, 1, 1) + struct.pack('<BBBBHHII', size, size, 0, 0, 1, 32, len(png), 22) + png)
print('Generated Windows icon')
