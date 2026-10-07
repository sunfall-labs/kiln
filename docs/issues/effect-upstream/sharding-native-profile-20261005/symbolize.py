import gzip, hashlib, json, struct, subprocess
from pathlib import Path
out = Path(__file__).resolve().parent

def text(path):
    with path.open('rb') as f:
        header = f.read(64)
        if header[:6] != b'\x7fELF\x02\x01':
            raise ValueError('expected little-endian ELF64')
        fields = struct.unpack('<16sHHIQQQIHHHHHH', header)
        shoff, shentsize, shnum, shstrndx = fields[6], fields[11], fields[12], fields[13]
        f.seek(shoff)
        sections = [struct.unpack('<IIQQQQIIQQ', f.read(shentsize)) for _ in range(shnum)]
        names = sections[shstrndx]
        f.seek(names[4]); strings = f.read(names[5])
        for section in sections:
            end = strings.find(b'\0', section[0])
            if strings[section[0]:end] == b'.text':
                f.seek(section[4]); remaining = section[5]; digest = hashlib.sha256()
                while remaining:
                    chunk = f.read(min(remaining, 1024 * 1024))
                    if not chunk: raise ValueError('short ELF section')
                    digest.update(chunk); remaining -= len(chunk)
                return {'address': section[3], 'size': section[5], 'sha256': digest.hexdigest()}
        raise ValueError('missing .text')

plain, symbolized = out/'native.bin', out/'native-symbolized.bin'
a, b = text(plain), text(symbolized)
if a != b: raise ValueError('text mismatch; cannot borrow symbolization')
raw = subprocess.check_output(['nm', '-S', '-n', '--defined-only', str(symbolized)], text=True)
(out/'native-symbols.txt').write_text(raw)
sidecar = json.loads((out/'profile.json.syms.json').read_text())
strings = sidecar['string_table']; entries = []
for line in raw.splitlines():
    parts = line.split()
    if len(parts) != 4 or parts[2] not in ('t', 'T', 'w', 'W'): continue
    address, size = int(parts[0], 16), int(parts[1], 16)
    if not size: continue
    name = parts[3]
    # Compact static identifiers repeat in different emitted translation units.
    if name.startswith('ku_'): name += '@0x' + parts[0]
    entries.append({'rva': address, 'size': size, 'symbol': len(strings)})
    strings.append(name)
entries.sort(key=lambda entry: entry['rva'])
matched = 0
for library in sidecar['data']:
    if library['debug_name'] == 'native.bin':
        library['symbol_table'] = entries; matched += 1
if matched != 1: raise ValueError('native library sidecar identity mismatch')
(out/'profile-symbolized.syms.json').write_text(json.dumps(sidecar))
receipt = {'text': a, 'text_identical': True, 'symbols': len(entries),
           'native_binary_sha256': hashlib.file_digest(plain.open('rb'), 'sha256').hexdigest(),
           'symbolized_binary_sha256': hashlib.file_digest(symbolized.open('rb'), 'sha256').hexdigest(),
           'sampler_handle':6923, 'sampler_exit_code':0,
           'symbolized_build_handle':64011, 'symbolized_build_exit_code':0,
           'source_head':'22e662a6f40f1260e9763b23055fa7a17f7f4eda',
           'cli_sha256':'525eb506b671ce38e378f2eb48e880554c8c973d1fea1545f558c62a65256454'}
(out/'receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps(receipt, indent=2))
