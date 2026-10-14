#!/usr/bin/env python3
"""Build one target once and reuse its wheel payload for archive and npm.

Exit 0 means the requested build, verification, or installation completed. Exit 1
means an input/artifact was refused or IO/build/install failed; stderr gives the
cause and recovery action. Argparse exits 2 for invalid command syntax.
"""
import argparse
import hashlib
import json
import os
import re
import sys
from pathlib import Path
import shutil
import struct
from email.parser import BytesParser
import subprocess
import tarfile
import tomllib
import zipfile


def run(*args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def target_facts():
    source = Path('scripts/npm-build.mjs').read_text()
    if 'const TARGETS = {' not in source:
        raise ValueError('cannot read the canonical npm target declarations')
    block = source.split('const TARGETS = {', 1)[1].split('\n};', 1)[0]
    facts = {target: {'platform': platform, 'arch': arch, 'exe': exe == 'true'}
             for target, platform, arch, exe in re.findall(
                 r'^\s+"([^"]+)":\s*\{\s*platform:\s*"([^"]+)",\s*arch:\s*"([^"]+)",\s*exe:\s*(true|false)', block, re.MULTILINE)}
    if not facts:
        raise ValueError('npm builder declares no supported targets')
    return facts


def supported_targets():
    return set(target_facts())


SEMVER_PATTERN = r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?'


def validate_version(version):
    if not isinstance(version, str) or not re.fullmatch(SEMVER_PATTERN, version):
        raise ValueError('invalid staged version')


def wheel_scripts(bundle, primary):
    scripts = [name for name in bundle.namelist() if '.data/scripts/' in name and not name.endswith('/')]
    names = [Path(name).name for name in scripts]
    if not scripts or len(set(names)) != len(names) or any(
            len(name.split('/')) != 3 or name.split('/')[1] != 'scripts' or
            not name.split('/')[0].endswith('.data') or
            not re.fullmatch(r'[A-Za-z0-9_.-]+', Path(name).name) or Path(name).name in ['.', '..']
            for name in scripts):
        raise ValueError('invalid wheel executable inventory')
    if primary not in names:
        raise ValueError('wheel omitted the primary executable')
    return scripts


def validate_wheel_members(wheel, version):
    """Refuse a wheel pip would install differently from what is verified here.

    A bin-bindings wheel installs nothing but its metadata and the executables
    under `.data/scripts`, the inventory `wheel_scripts` reconciles.
    """
    names = wheel.namelist()
    if len(set(names)) != len(names):
        raise ValueError('wheel repeats a member')
    if len({name.split('/')[0] for name in names if name.split('/')[0].endswith('.dist-info')}) != 1:
        raise ValueError('wheel must carry exactly one .dist-info directory')
    stem = f'onepipeline_cli-{version}'
    for info in wheel.infolist():
        parts = info.filename.rstrip('/').split('/')
        if (info.filename.startswith('/') or '\\' in info.filename or
                any(part in ['', '.', '..'] for part in parts) or
                parts[0] not in [f'{stem}.dist-info', f'{stem}.data'] or
                (parts[0] == f'{stem}.data' and len(parts) > 1 and parts[1] != 'scripts')):
            raise ValueError(f'unsafe wheel member: {info.filename}')
        if not info.is_dir() and (info.external_attr >> 16) & 0o170000 not in (0, 0o100000):
            raise ValueError(f'wheel member is not a regular file: {info.filename}')


def validate_wheel_filename(path, version, tags):
    """The filename is what an installer selects by, so it must say what WHEEL says."""
    found = re.fullmatch(r'onepipeline_cli-([^-]+)-([^-]+)-([^-]+)-([^-]+)\.whl', Path(path).name)
    if not found or found[1] != version:
        raise ValueError('wheel filename name/version mismatch')
    advertised = {f'{py}-{abi}-{plat}' for py in found[2].split('.')
                  for abi in found[3].split('.') for plat in found[4].split('.')}
    declared = {line[len('Tag: '):] for line in tags.splitlines() if line.startswith('Tag: ')}
    if advertised != declared:
        raise ValueError('wheel filename tags differ from its WHEEL tags')


def dist_info_member(wheel, name):
    members = [member for member in wheel.namelist()
               if member.endswith('.dist-info/' + name) and member.count('/') == 1]
    if len(members) != 1:
        raise ValueError(f'wheel is missing its .dist-info/{name}')
    return members[0]


def tar_payload(bundle, name):
    member = bundle.getmember(name)
    if not member.isfile():
        raise ValueError(f'archive member is not a regular file: {name}')
    return bundle.extractfile(member).read()


def validate_npm_archive(bundle, binary_name):
    files = {'package/README.md', 'package/package.json', f'package/bin/{binary_name}'}
    directories = {'package', 'package/bin'}
    names = set()
    for member in bundle.getmembers():
        name = member.name.rstrip('/')
        if name in names or name.startswith('/') or '\\' in name or any(part in ['.', '..', ''] for part in name.split('/')):
            raise ValueError('unsafe or duplicate npm archive member')
        names.add(name)
        if member.isdir() and name in directories:
            continue
        if not member.isfile() or name not in files:
            raise ValueError('unexpected npm archive member')
        if name != f'package/bin/{binary_name}' and member.mode & 0o111:
            raise ValueError('unexpected executable npm archive member')
    if not files.issubset(names):
        raise ValueError('npm archive omitted required members')


def validate_target(target):
    if not isinstance(target, str) or target not in supported_targets():
        raise ValueError('unsupported target; choose one of: ' + ', '.join(sorted(supported_targets())))


def validate_record(record):
    fields = {'schema_version', 'tag', 'target', 'version', 'binary_sha256', 'wheel', 'archive', 'npm', 'executables'}
    if not isinstance(record, dict) or set(record) != fields:
        raise ValueError('staged record has missing or unknown fields')
    if type(record['schema_version']) is not int or record['schema_version'] != 1:
        raise ValueError('unsupported staged record schema')
    validate_target(record['target'])
    for field in ['tag', 'version', 'binary_sha256', 'wheel', 'archive', 'npm']:
        if not isinstance(record[field], str) or not record[field]:
            raise ValueError(f'invalid staged record field: {field}')
    if not re.fullmatch(r'[0-9a-f]{64}', record['binary_sha256']):
        raise ValueError('invalid payload hash')
    validate_version(record['version'])
    inventory = record['executables']
    if not isinstance(inventory, list) or not inventory or any(
            not isinstance(name, str) or not name or Path(name).name != name or name in ['.', '..'] or '\\' in name
            for name in inventory):
        raise ValueError('invalid executable inventory')


def build(target, out, profile, tag=None):
    validate_target(target)
    package = tomllib.loads(Path('Cargo.toml').read_text()).get('package')
    if not isinstance(package, dict):
        raise ValueError('Cargo.toml has no package metadata')
    version = package.get('version')
    validate_version(version)
    if tag is not None and tag != f'v{version}':
        raise ValueError('release tag differs from Cargo.toml version')
    out = out.resolve()
    wheels = out / 'wheels' / target
    wheels.mkdir(parents=True, exist_ok=True)
    # Maturin compiles with Cargo and retains its generated CycloneDX SBOM.
    # Its final stripped payload is the single binary used by every other form.
    run('maturin', 'build', '--locked', '--profile', profile, '--target', target,
        '--out', str(wheels))
    wheel, = wheels.glob('*.whl')
    stage = out / 'stage' / target
    stage.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(wheel) as bundle:
        bins = wheel_scripts(bundle, 'onepipeline.exe' if target_facts()[target]['exe'] else 'onepipeline')
        for name in bins:
            binary = stage / Path(name).name
            binary.write_bytes(bundle.read(name))
            binary.chmod(0o755)
        if not any('.dist-info/sboms/' in n for n in bundle.namelist()):
            raise ValueError('wheel omitted the SBOM')
    for name in ['README.md', 'LICENSE', 'CHANGELOG.md']:
        shutil.copyfile(name, stage / name)
    archives = out / 'archives'
    archives.mkdir(exist_ok=True)
    archive = archives / f'onepipeline-v{version}-{target}.tar.gz'
    with tarfile.open(archive, 'w:gz') as bundle:
        for path in sorted(stage.iterdir()):
            bundle.add(path, arcname=path.name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (archives / f'onepipeline-v{version}-{target}.sha256').write_text(f'{digest}  {archive.name}\n')
    binary = stage / ('onepipeline.exe' if 'windows' in target else 'onepipeline')
    package = subprocess.check_output(['node', 'scripts/npm-build.mjs', 'platform', '--target',
        target, '--binary', str(binary), '--out', str(out / 'npm')], text=True).strip()
    tarballs = out / 'npm-artifacts' / target
    tarballs.mkdir(parents=True, exist_ok=True)
    run('npm', 'pack', '--pack-destination', str(tarballs), cwd=package)
    tgz, = tarballs.glob('*.tgz')
    with tarfile.open(tgz) as bundle:
        npm_binary = tar_payload(bundle, f'package/bin/{binary.name}')
    payload = binary.read_bytes()
    if npm_binary != payload:
        raise ValueError('npm payload differs from the wheel/archive input')
    record = {'schema_version': 1, 'tag': f'v{version}', 'target': target, 'version': version, 'binary_sha256': hashlib.sha256(payload).hexdigest(),
        'wheel': str(wheel), 'archive': str(archive), 'npm': str(tgz), 'executables': [Path(n).name for n in bins]}
    (out / f'{target}.json').write_text(json.dumps(record, indent=2) + '\n')
    verify(record, quiet=True)
    for source, directory in [(wheel, 'publish-wheels'), (tgz, 'publish-npm')]:
        publication = out / directory
        publication.mkdir(exist_ok=True)
        shutil.copyfile(source, publication / source.name)
    return record


def verify(record, quiet=False):
    validate_record(record)
    target = record['target']
    version = record['version']
    if record['tag'] != f'v{version}':
        raise ValueError('record tag/version mismatch')
    binary_name = 'onepipeline.exe' if 'windows' in target else 'onepipeline'
    with zipfile.ZipFile(record['wheel']) as wheel:
        validate_wheel_members(wheel, version)
        scripts = wheel_scripts(wheel, binary_name)
        if sorted(Path(n).name for n in scripts) != sorted(record['executables']):
            raise ValueError('executable inventory differs from wheel')
        payload = wheel.read(next(n for n in scripts if Path(n).name == binary_name))
        metadata = BytesParser().parsebytes(wheel.read(dist_info_member(wheel, 'METADATA')))
        if metadata['Name'] != 'onepipeline-cli' or metadata['Version'] != version:
            raise ValueError('wheel name/version mismatch')
        tags = wheel.read(dist_info_member(wheel, 'WHEEL')).decode()
        arch = 'arm64' if 'apple' in target and target.startswith('aarch64') else 'aarch64' if target.startswith('aarch64') else 'amd64' if 'windows' in target else 'x86_64'
        platforms = ['macosx_'] if 'apple' in target else ['win_'] if 'windows' in target else ['manylinux', 'musllinux', 'linux_']
        if not any(line.startswith('Tag: ') and line.endswith(arch) and
                   any(line.rsplit('-', 1)[-1].startswith(platform) for platform in platforms)
                   for line in tags.splitlines()):
            raise ValueError('wheel architecture tag mismatch')
        for name in [n for n in wheel.namelist() if '.dist-info/sboms/' in n]:
            bom = json.loads(wheel.read(name))
            if not isinstance(bom, dict) or bom.get('bomFormat') != 'CycloneDX':
                raise ValueError('invalid SBOM')
        if not any('.dist-info/sboms/' in n for n in wheel.namelist()):
            raise ValueError('missing SBOM')
        if 'apple-darwin' in target:
            expected = 0x0100000c
            if len(payload) < 8 or payload[:4] != b'\xcf\xfa\xed\xfe' or struct.unpack('<I', payload[4:8])[0] != expected:
                raise ValueError('Mach-O architecture mismatch')
        validate_wheel_filename(record['wheel'], version, tags)
        with tarfile.open(record['archive']) as archive:
            expected_names = sorted(record['executables'] + ['README.md', 'LICENSE', 'CHANGELOG.md'])
            if sorted(archive.getnames()) != expected_names:
                raise ValueError('archive layout mismatch')
            for member in archive.getmembers():
                if not member.isfile():
                    raise ValueError(f'archive member is not a regular file: {member.name}')
            for name in scripts:
                if tar_payload(archive, Path(name).name) != wheel.read(name):
                    raise ValueError('archive executable differs from wheel payload')
        with tarfile.open(record['npm']) as npm:
            validate_npm_archive(npm, binary_name)
            if tar_payload(npm, f'package/bin/{binary_name}') != payload:
                raise ValueError('npm executable differs from wheel payload')
            package = json.loads(tar_payload(npm, 'package/package.json'))
            if not isinstance(package, dict):
                raise ValueError('invalid npm manifest object')
            if package.get('version') != version:
                raise ValueError('npm version mismatch')
            facts = target_facts()[target]
            if package.get('name') != f"onepipeline-cli-{facts['platform']}-{facts['arch']}" or package.get('os') != [facts['platform']] or package.get('cpu') != [facts['arch']]:
                raise ValueError('npm target identity mismatch')
        if hashlib.sha256(payload).hexdigest() != record['binary_sha256']:
            raise ValueError('payload hash mismatch')
    archive = Path(record['archive'])
    expected_name = f'onepipeline-v{version}-{target}'
    if archive.name != expected_name + '.tar.gz':
        raise ValueError('archive name mismatch')
    checksum = archive.parent / (expected_name + '.sha256')
    if checksum.read_text() != f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n':
        raise ValueError('archive checksum mismatch')
    if not quiet:
        print(f'verified binary reuse and metadata for {target}')


def install(record, out, python, node):
    """Native installs of the staged wheel and npm packages with the host's runtimes."""
    prefix = out.resolve() / 'install' / record['target']
    run(python, '-m', 'venv', str(prefix / 'python'))
    scripts = 'Scripts' if os.name == 'nt' else 'bin'
    pip = prefix / 'python' / scripts / ('pip.exe' if os.name == 'nt' else 'pip')
    interpreter = prefix / 'python' / scripts / ('python.exe' if os.name == 'nt' else 'python')
    run(str(interpreter), '-m', 'pip', 'install', '--no-index', record['wheel'])
    env = dict(os.environ, PATH=f"{pip.parent}{os.pathsep}{os.environ['PATH']}")
    run('bash', 'scripts/smoke-published.sh', '--expect-version', record['version'],
        '--label', f"staged wheel {record['target']}", env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    # llmlint: ignore-block[work_goes_through_command_surface] This release builder composes the same primitive as the platform builders in a per-target staging directory the Nx target's fixed output cannot express; scripts/nx.sh keeps Nx out of the runtime dependencies of its scripts.
    launcher = subprocess.check_output(['node', 'scripts/npm-build.mjs', 'launcher', '--out',
        str(prefix / 'launcher')], text=True).strip()
    # llmlint: ignore-end[work_goes_through_command_surface]
    run('npm', 'pack', '--pack-destination', str(prefix), cwd=launcher)
    launcher_tgz, = prefix.glob('onepipeline-cli-*.tgz')
    # Install the real platform tarball beside the packed launcher offline;
    # the launcher resolves that local platform package when it executes.
    run('npm', 'install', '--prefix', str(prefix / 'npm'), '--ignore-scripts', '--offline',
        '--no-audit', '--no-fund', '--force', '--omit=optional', record['npm'], str(launcher_tgz))
    launcher_bin = prefix / 'npm' / 'node_modules' / 'onepipeline-cli' / 'bin' / 'onepipeline.js'
    wrapper = prefix / 'bin'
    wrapper.mkdir(parents=True, exist_ok=True)
    command = wrapper / 'onepipeline'
    import shlex
    command.write_text('#!/bin/sh\nexec ' + shlex.quote(node) + ' ' + shlex.quote(str(launcher_bin)) + ' "$@"\n')
    command.chmod(0o755)
    env['PATH'] = f"{wrapper}{os.pathsep}{os.environ['PATH']}"
    run('bash', 'scripts/smoke-published.sh', '--expect-version', record['version'],
        '--label', f"staged npm {record['target']}", env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    print(f"staged-release: installed wheel and npm for {record['target']} {record['version']}")


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--verify', type=Path)
    parser.add_argument('--tag')
    parser.add_argument('--target')
    parser.add_argument('--out', type=Path)
    parser.add_argument('--profile', default='release', choices=['release', 'dev'])
    parser.add_argument('--install', action='store_true')
    parser.add_argument('--python', default='python3')
    parser.add_argument('--node', default=shutil.which('node'))
    args = parser.parse_args()
    try:
        if args.verify:
            record = json.loads(args.verify.read_text())
            verify(record, quiet=args.install)
            if args.install:
                if not args.out:
                    parser.error('--out is required for staged installation')
                install(record, args.out, args.python, args.node)
        else:
            if not args.target or not args.out:
                parser.error('--target and --out are required when building')
            record = build(args.target, args.out, args.profile, args.tag)
            if args.install:
                install(record, args.out, args.python, args.node)
            else:
                print(f'verified binary reuse and metadata for {args.target}')
    except (ValueError, OSError, KeyError, StopIteration, zipfile.BadZipFile, tarfile.TarError,
            subprocess.SubprocessError, EOFError, RuntimeError) as error:
        if isinstance(error, subprocess.CalledProcessError):
            for output in [error.stdout, error.stderr]:
                if output:
                    print(output.decode() if isinstance(output, bytes) else output, file=sys.stderr, end='')
        print(f'staged-release: {error}', file=sys.stderr)
        print('next: correct the input or damaged artifact, then regenerate with '
              'scripts/build-staged-release.py --target <supported-triple> --out <empty-directory>; '
              'provision maturin with just nx run onepipeline-npm:bootstrap.', file=sys.stderr)
        raise SystemExit(1)
