"""Mutate real staged bundles to drive the release verifier's rejection boundary."""
import io
import json
from pathlib import Path
import shutil
import struct
import sys
import tarfile
import zipfile

record_path, case, destination = sys.argv[1:]
record = json.loads(Path(record_path).read_text())
out = Path(destination)
out.mkdir(parents=True, exist_ok=True)

if case.startswith('version-invalid-'):
    versions = ['01.2.3', '1.2.3-.', '1.2.3+.', '1.2.3-a..b', '1.2.3-01', '1.2.3\n']
    record['version'] = versions[int(case.removeprefix('version-invalid-'))]
elif case == 'installed-wheel-version':
    previous = record['version']; record['version'] = '0.0.0'; record['tag'] = 'v0.0.0'
    wheel = out / Path(record['wheel']).name.replace(previous, '0.0.0')
    with zipfile.ZipFile(record['wheel']) as original, zipfile.ZipFile(wheel, 'w') as changed:
        for entry in original.infolist():
            payload = original.read(entry)
            if entry.filename.endswith('/METADATA'): payload = payload.replace(f'Version: {previous}'.encode(), b'Version: 0.0.0')
            entry.filename = entry.filename.replace(previous, '0.0.0'); changed.writestr(entry, payload)
    record['wheel'] = str(wheel)
    archive = out / f"onepipeline-v0.0.0-{record['target']}.tar.gz"
    shutil.copyfile(record['archive'], archive); record['archive'] = str(archive)
    import hashlib
    (out / f"onepipeline-v0.0.0-{record['target']}.sha256").write_text(f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n')
    tgz = out / Path(record['npm']).name
    with tarfile.open(record['npm']) as original, tarfile.open(tgz, 'w:gz') as changed:
        for entry in original.getmembers():
            payload = original.extractfile(entry).read() if entry.isfile() else None
            if entry.name == 'package/package.json':
                manifest = json.loads(payload); manifest['version'] = '0.0.0'; payload = json.dumps(manifest).encode()
            if payload is not None: entry.size = len(payload)
            changed.addfile(entry, io.BytesIO(payload) if payload is not None else None)
    record['npm'] = str(tgz)
elif case in ['schema', 'tag', 'unknown', 'missing', 'field-type', 'target', 'inventory', 'hash-syntax', 'inventory-empty', 'inventory-type', 'inventory-unsafe']:
    if case == 'schema': record['schema_version'] = 2
    if case == 'tag': record['tag'] = 'v0.0.0'
    if case == 'unknown': record['unexpected'] = True
    if case == 'missing': del record['wheel']
    if case == 'field-type': record['version'] = 42
    if case == 'target': record['target'] = '../../escape'
    if case == 'hash-syntax': record['binary_sha256'] = 'not-a-hash'
    if case == 'inventory-empty': record['executables'] = []
    if case == 'inventory-type': record['executables'] = [42]
    if case == 'inventory-unsafe': record['executables'] = ['../escape']
    if case == 'inventory': record['executables'].append('missing-helper')
elif case in ['wheel-version', 'wheel-arch', 'sbom', 'sbom-object', 'no-sbom', 'macho', 'wheel-duplicate', 'wheel-path', 'wheel-primary', 'no-metadata', 'no-wheel-tags', 'wheel-traversal', 'wheel-symlink', 'wheel-dist-info', 'wheel-data-scheme', 'wheel-filename', 'wheel-repeat', 'wheel-filename-version', 'wheel-extra-tag']:
    source = record['wheel']
    target = out / Path(source).name
    with zipfile.ZipFile(source) as original, zipfile.ZipFile(target, 'w') as changed:
        for entry in original.infolist():
            payload = original.read(entry)
            if case == 'no-sbom' and '/sboms/' in entry.filename: continue
            if case == 'no-metadata' and entry.filename.endswith('.dist-info/METADATA'): continue
            if case == 'no-wheel-tags' and entry.filename.endswith('.dist-info/WHEEL'): continue
            if case == 'sbom-object' and '/sboms/' in entry.filename: payload = b'[]'
            if case == 'sbom' and '/sboms/' in entry.filename: payload = b'{"bomFormat":"wrong"}'
            if case == 'wheel-version' and entry.filename.endswith('/METADATA'):
                payload = payload.replace(f"Version: {record['version']}".encode(), b'Version: 0.0.0')
            if case == 'wheel-extra-tag' and entry.filename.endswith('.dist-info/WHEEL'):
                payload += b'Tag: py3-none-win_amd64\n'
            if case == 'wheel-arch' and entry.filename.endswith('/WHEEL'):
                payload = b'Wheel-Version: 1.0\nTag: py3-none-wrong_other\n'
            if case == 'macho':
                # An Apple silicon wheel whose executable is an x86_64 Mach-O.
                record['target'] = 'aarch64-apple-darwin'
                if entry.filename.endswith('/WHEEL'): payload = b'Wheel-Version: 1.0\nTag: py3-none-macosx_11_0_arm64\n'
                if '.data/scripts/' in entry.filename:
                    payload = b'\xcf\xfa\xed\xfe' + struct.pack('<I', 0x01000007) + payload[8:]
            if case == 'macho' and '.data/scripts/' in entry.filename:
                entry.filename = entry.filename.removesuffix('.exe')
                record['executables'] = ['onepipeline']
            if case == 'wheel-primary' and '.data/scripts/' in entry.filename:
                entry.filename = entry.filename.replace('/onepipeline', '/other-helper')
            if case == 'wheel-path' and '.data/scripts/' in entry.filename:
                entry.filename = entry.filename.replace('/scripts/', '/scripts/nested/')
            changed.writestr(entry, payload)
            if case == 'wheel-traversal' and entry.filename.endswith('.dist-info/WHEEL'):
                changed.writestr('../escape', b'outside the install prefix')
            if case == 'wheel-symlink' and entry.filename.endswith('.dist-info/WHEEL'):
                link = zipfile.ZipInfo(entry.filename.split('/')[0] + '/link')
                link.external_attr = (0o120777 << 16)
                changed.writestr(link, b'/etc/passwd')
            if case == 'wheel-repeat' and entry.filename.endswith('.dist-info/WHEEL'):
                changed.writestr(entry.filename, payload)
            if case == 'wheel-data-scheme' and '.data/scripts/' in entry.filename:
                changed.writestr(entry.filename.replace('/scripts/', '/data/bin/'), payload)
            if case == 'wheel-dist-info' and entry.filename.endswith('.dist-info/WHEEL'):
                changed.writestr('other-1.0.dist-info/METADATA', b'Name: other\n')
            if case == 'wheel-duplicate' and '.data/scripts/' in entry.filename:
                changed.writestr('duplicate.data/scripts/' + Path(entry.filename).name, payload)
    if case == 'wheel-filename':
        stem, version, rest = target.name.split('-', 2)
        target = target.rename(target.with_name(f'{stem}-{version}-py3-none-win_amd64.whl'))
    if case == 'wheel-filename-version':
        stem, version, rest = target.name.split('-', 2)
        target = target.rename(target.with_name(f'{stem}-0.0.0-{rest}'))
    record['wheel'] = str(target)
elif case in ['archive-layout', 'archive-payload', 'npm-payload', 'npm-version', 'npm-object', 'npm-directory', 'npm-name', 'npm-os', 'npm-cpu', 'archive-directory', 'npm-unsafe', 'npm-duplicate', 'npm-link', 'npm-executable', 'npm-unexpected', 'npm-missing', 'archive-doc-directory', 'archive-doc-symlink', 'archive-doc-hardlink', 'npm-mode', 'archive-mode']:
    key = 'npm' if case.startswith('npm-') else 'archive'
    source = record[key]
    target = out / Path(source).name
    with tarfile.open(source) as original, tarfile.open(target, 'w:gz') as changed:
        for entry in original.getmembers():
            if case == 'npm-missing' and entry.name == 'package/README.md': continue
            if case == 'archive-layout' and entry.name == 'LICENSE': continue
            payload = original.extractfile(entry).read() if entry.isfile() else None
            if payload is not None:
                if case == 'archive-payload' and entry.name in record['executables']: payload += b'corruption'
                if case == 'npm-payload' and '/bin/' in entry.name: payload += b'corruption'
                if case == 'npm-object' and entry.name == 'package/package.json': payload = b'[]'
                if case in ['npm-version', 'npm-name', 'npm-os', 'npm-cpu'] and entry.name == 'package/package.json':
                    manifest = json.loads(payload)
                    if case == 'npm-version': manifest['version'] = '0.0.0'
                    if case == 'npm-name': manifest['name'] = 'wrong-package'
                    if case == 'npm-os': manifest['os'] = ['wrong-os']
                    if case == 'npm-cpu': manifest['cpu'] = ['wrong-cpu']
                    payload = json.dumps(manifest).encode()
                entry.size = len(payload)
                if (case == 'npm-directory' and '/bin/' in entry.name) or (case == 'archive-directory' and entry.name in record['executables']):
                    entry.type = tarfile.DIRTYPE; entry.size = 0; payload = None
            documentation = {'archive-doc-directory': 'README.md', 'archive-doc-symlink': 'LICENSE', 'archive-doc-hardlink': 'CHANGELOG.md'}
            if case in documentation and entry.name == documentation[case]:
                entry.type = {'archive-doc-directory': tarfile.DIRTYPE, 'archive-doc-symlink': tarfile.SYMTYPE, 'archive-doc-hardlink': tarfile.LNKTYPE}[case]
                entry.linkname = '../../escape'; entry.size = 0; payload = None
            if case == 'npm-executable' and entry.name == 'package/README.md': entry.mode |= 0o111
            if case == 'npm-mode' and entry.name.startswith('package/bin/'): entry.mode &= ~0o111
            if case == 'archive-mode' and entry.name in record['executables']: entry.mode &= ~0o111
            changed.addfile(entry, io.BytesIO(payload) if payload is not None else None)
            if case == 'npm-duplicate' and entry.name == 'package/package.json': changed.addfile(entry, io.BytesIO(payload))
        if case in ['npm-unsafe', 'npm-link', 'npm-unexpected']:
            extra = tarfile.TarInfo('../escape' if case == 'npm-unsafe' else 'package/extra')
            if case == 'npm-link': extra.type = tarfile.SYMTYPE; extra.linkname = '../../escape'
            else: extra.size = 3
            changed.addfile(extra, io.BytesIO(b'bad') if case != 'npm-link' else None)
    record[key] = str(target)
    if case.startswith('archive-doc-'):
        import hashlib
        (out / f"onepipeline-v{record['version']}-{record['target']}.sha256").write_text(f'{hashlib.sha256(target.read_bytes()).hexdigest()}  {target.name}\n')
elif case == 'wheel-encrypted-flagged':
    import struct
    source = Path(record['wheel']); target = out / source.name
    payload = bytearray(source.read_bytes())
    with zipfile.ZipFile(source) as bundle:
        for entry in bundle.infolist():
            offset = entry.header_offset + 6
            flags, = struct.unpack_from('<H', payload, offset)
            struct.pack_into('<H', payload, offset, flags | 1)
        directory = bundle.start_dir
        for entry in bundle.infolist():
            flags, = struct.unpack_from('<H', payload, directory + 8)
            struct.pack_into('<H', payload, directory + 8, flags | 1)
            name, extra, comment = struct.unpack_from('<HHH', payload, directory + 28)
            directory += 46 + name + extra + comment
    target.write_bytes(payload); record['wheel'] = str(target)
elif case == 'archive-truncated':
    source = Path(record['archive']); target = out / source.name
    target.write_bytes(source.read_bytes()[:-100]); record['archive'] = str(target)
elif case in ['archive-name', 'checksum']:
    source = Path(record['archive'])
    target = out / ('wrong.tar.gz' if case == 'archive-name' else source.name)
    shutil.copyfile(source, target)
    record['archive'] = str(target)
    (out / f"onepipeline-v{record['version']}-{record['target']}.sha256").write_text('damaged checksum\n')
else:
    raise ValueError(f'unknown mutation: {case}')
path = out / 'record.json'
path.write_text(json.dumps(record))
print(path)
