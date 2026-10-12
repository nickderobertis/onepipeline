#!/usr/bin/env python3
"""Build one target once and reuse its wheel payload for archive and npm."""
import argparse
import hashlib
import json
import os
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


def build(target, out, profile, tag=None):
    version = tomllib.loads(Path('Cargo.toml').read_text())['package']['version']
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
        bins = [n for n in bundle.namelist() if '.data/scripts/' in n and not n.endswith('/')]
        if not bins:
            raise ValueError('wheel carries no executable')
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
        npm_binary = bundle.extractfile(f'package/bin/{binary.name}').read()
    payload = binary.read_bytes()
    if npm_binary != payload:
        raise ValueError('npm payload differs from the wheel/archive input')
    record = {'schema_version': 1, 'tag': f'v{version}', 'target': target, 'version': version, 'binary_sha256': hashlib.sha256(payload).hexdigest(),
        'wheel': str(wheel), 'archive': str(archive), 'npm': str(tgz), 'executables': [Path(n).name for n in bins]}
    (out / f'{target}.json').write_text(json.dumps(record, indent=2) + '\n')
    verify(record)
    return record


def verify(record):
    if record['schema_version'] != 1:
        raise ValueError('unsupported staged record schema')
    target = record['target']
    version = record['version']
    if record['tag'] != f'v{version}':
        raise ValueError('record tag/version mismatch')
    binary_name = 'onepipeline.exe' if 'windows' in target else 'onepipeline'
    with zipfile.ZipFile(record['wheel']) as wheel:
        scripts = [n for n in wheel.namelist() if '.data/scripts/' in n and not n.endswith('/')]
        if sorted(Path(n).name for n in scripts) != sorted(record['executables']):
            raise ValueError('executable inventory differs from wheel')
        payload = wheel.read(next(n for n in scripts if Path(n).name == binary_name))
        metadata = BytesParser().parsebytes(wheel.read(next(n for n in wheel.namelist() if n.endswith('/METADATA'))))
        if metadata['Name'] != 'onepipeline-cli' or metadata['Version'] != version:
            raise ValueError('wheel name/version mismatch')
        tags = wheel.read(next(n for n in wheel.namelist() if n.endswith('/WHEEL'))).decode()
        arch = 'arm64' if 'apple' in target and target.startswith('aarch64') else 'aarch64' if target.startswith('aarch64') else 'amd64' if 'windows' in target else 'x86_64'
        if not any(line.startswith('Tag: ') and line.endswith(arch) for line in tags.splitlines()):
            raise ValueError('wheel architecture tag mismatch')
        for name in [n for n in wheel.namelist() if '.dist-info/sboms/' in n]:
            if json.loads(wheel.read(name))['bomFormat'] != 'CycloneDX':
                raise ValueError('invalid SBOM')
        if not any('.dist-info/sboms/' in n for n in wheel.namelist()):
            raise ValueError('missing SBOM')
        if 'apple-darwin' in target:
            expected = 0x0100000c if target.startswith('aarch64') else 0x01000007
            if payload[:4] != b'\xcf\xfa\xed\xfe' or struct.unpack('<I', payload[4:8])[0] != expected:
                raise ValueError('Mach-O architecture mismatch')
        with tarfile.open(record['archive']) as archive:
            expected_names = sorted(record['executables'] + ['README.md', 'LICENSE', 'CHANGELOG.md'])
            if sorted(archive.getnames()) != expected_names:
                raise ValueError('archive layout mismatch')
            for name in scripts:
                if archive.extractfile(Path(name).name).read() != wheel.read(name):
                    raise ValueError('archive executable differs from wheel payload')
        with tarfile.open(record['npm']) as npm:
            if npm.extractfile(f'package/bin/{binary_name}').read() != payload:
                raise ValueError('npm executable differs from wheel payload')
            package = json.load(npm.extractfile('package/package.json'))
            if package['version'] != version:
                raise ValueError('npm version mismatch')
        if hashlib.sha256(payload).hexdigest() != record['binary_sha256']:
            raise ValueError('payload hash mismatch')
    archive = Path(record['archive'])
    expected_name = f'onepipeline-v{version}-{target}'
    if archive.name != expected_name + '.tar.gz':
        raise ValueError('archive name mismatch')
    checksum = archive.parent / (expected_name + '.sha256')
    if checksum.read_text() != f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n':
        raise ValueError('archive checksum mismatch')
    print(f'verified binary reuse and metadata for {target}')


def install(record, out, python, node):
    """Native installs, including an x86_64 Python/Node under Rosetta on Darwin."""
    prefix = out.resolve() / 'install' / record['target']
    run(python, '-m', 'venv', str(prefix / 'python'))
    pip = prefix / 'python' / 'bin' / 'pip'
    interpreter = prefix / 'python' / 'bin' / 'python'
    python_command = ['arch', '-x86_64', str(interpreter)] if record['target'] == 'x86_64-apple-darwin' else [str(interpreter)]
    run(*python_command, '-m', 'pip', 'install', '--no-index', record['wheel'])
    env = dict(os.environ, PATH=f"{pip.parent}{os.pathsep}{os.environ['PATH']}")
    run('bash', 'scripts/smoke-published.sh', '--expect-version', record['version'],
        '--label', f"staged wheel {record['target']}", env=env)
    launcher = subprocess.check_output(['node', 'scripts/npm-build.mjs', 'launcher', '--out',
        str(prefix / 'launcher')], text=True).strip()
    run('npm', 'pack', '--pack-destination', str(prefix), cwd=launcher)
    launcher_tgz, = prefix.glob('onepipeline-cli-*.tgz')
    # Local platform and launcher together: npm resolves the real optional dependency
    # without publication; only registry transport is absent from this journey.
    run('npm', 'install', '--prefix', str(prefix / 'npm'), '--ignore-scripts', '--offline',
        '--no-audit', '--no-fund', '--force', '--omit=optional', record['npm'], str(launcher_tgz))
    launcher_bin = prefix / 'npm' / 'node_modules' / 'onepipeline-cli' / 'bin' / 'onepipeline.js'
    run(node, str(launcher_bin), '--version')
    # Smoke drives the launcher through its real executable, including failures.
    wrapper = prefix / 'bin'
    wrapper.mkdir(parents=True, exist_ok=True)
    command = wrapper / 'onepipeline'
    import shlex
    command.write_text('#!/bin/sh\nexec ' + shlex.quote(node) + ' ' + shlex.quote(str(launcher_bin)) + ' "$@"\n')
    command.chmod(0o755)
    env['PATH'] = f"{wrapper}{os.pathsep}{os.environ['PATH']}"
    run('bash', 'scripts/smoke-published.sh', '--expect-version', record['version'],
        '--label', f"staged npm {record['target']}", env=env)


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
    if args.verify:
        verify(json.loads(args.verify.read_text()))
        raise SystemExit(0)
    if not args.target or not args.out:
        parser.error('--target and --out are required when building')
    record = build(args.target, args.out, args.profile, args.tag)
    if args.install:
        install(record, args.out, args.python, args.node)
