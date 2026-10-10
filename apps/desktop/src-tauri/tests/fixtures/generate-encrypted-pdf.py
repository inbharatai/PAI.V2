"""Generate our deterministic PDF Standard Security revision-2 regression fixture.
Test-only password: fixture-user. No external dependencies or real source data.
Run from any directory; writes encrypted-text.pdf beside this script.
"""
from hashlib import md5
from pathlib import Path
import struct

# Explicit canonical bytes avoid ambiguity in a hand-transcribed hex string.
PAD = bytes([0x28,0xbf,0x4e,0x5e,0x4e,0x75,0x8a,0x41,0x64,0,0x4e,0x56,0xff,0xfa,1,8,0x2e,0x2e,0,0xb6,0xd0,0x68,0x3e,0x80,0x2f,0x0c,0xa9,0xfe,0x64,0x53,0x69,0x7a])
def padded(password):
    return (password + PAD)[:32]
def rc4(key, data):
    state = list(range(256)); j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]
    i = j = 0; out = bytearray()
    for value in data:
        i = (i + 1) % 256; j = (j + state[i]) % 256
        state[i], state[j] = state[j], state[i]
        out.append(value ^ state[(state[i] + state[j]) % 256])
    return bytes(out)

file_id = md5(b'unoone-hermetic-encrypted-fixture-v1').digest()
owner = rc4(md5(padded(b'fixture-owner')).digest()[:5], padded(b'fixture-user'))
permissions = -4
key = md5(padded(b'fixture-user') + owner + struct.pack('<i', permissions) + file_id).digest()[:5]
user = rc4(key, PAD)
stream_key = md5(key + bytes([5, 0, 0, 0, 0])).digest()[:10]
stream = rc4(stream_key, b'BT /F1 12 Tf (Encrypted fixture text) Tj ET')
objects = [
    b'<< /Type /Catalog /Pages 2 0 R >>',
    b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>',
    b'<< /Length ' + str(len(stream)).encode() + b' >>\nstream\n' + stream + b'\nendstream',
    b'<< /Filter /Standard /V 1 /R 2 /O <' + owner.hex().encode() + b'> /U <' + user.hex().encode() + b'> /P -4 >>',
]
data = bytearray(b'%PDF-1.4\n%\xe2\xe3\xcf\xd3\n'); offsets = [0]
for i, value in enumerate(objects, 1):
    offsets.append(len(data)); data.extend(f'{i} 0 obj\n'.encode() + value + b'\nendobj\n')
xref = len(data)
data.extend(b'xref\n0 7\n0000000000 65535 f \n')
for offset in offsets[1:]: data.extend(f'{offset:010d} 00000 n \n'.encode())
data.extend(b'trailer\n<< /Size 7 /Root 1 0 R /Encrypt 6 0 R /ID [<' + file_id.hex().encode() + b'><' + file_id.hex().encode() + b'>] >>\n')
data.extend(f'startxref\n{xref}\n%%EOF\n'.encode())
Path(__file__).with_name('encrypted-text.pdf').write_bytes(data)
