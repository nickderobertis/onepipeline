#!/usr/bin/env python3
"""Release onepipeline one platform at a time, then the platform-independent rest.

release.yml's three build jobs and its `publish` job are each one call of this
through the `just release-<platform>` / `just release-publish` recipes, so a
release and a developer drive the same steps. What each phase does, and in what
order, is AGENTS.md's "Commits, releases, and merging".

Usage:
  release.py plan PLATFORM [--version V]       PLATFORM: linux, macos, windows or common
  release.py build PLATFORM [--out DIR]        build each target once, package, test-install
  release.py publish PLATFORM [--out DIR] [--dry-run]
  release.py verify PLATFORM [--out DIR]       install from the registries and run it
  release.py dry-run PLATFORM [--out DIR]      build, then publish --dry-run
  release.py publish-common [--out DIR] [--dry-run]
  release.py outcome [--out DIR]

`plan` lists, without building, every file, package, wheel tag and target a
platform's build produces or `publish` uploads; `build` refuses an artifact that
differs from it, so the plan is what a release publishes.

The release reaches this as environment, which the workflow sets:
  RELEASE_TAG             the Release's tag (default: v<Cargo.toml version>, dry runs only)
  PYPI_PUBLISH, NPM_PUBLISH   'true' turns that registry on; PYPI_TOKEN, NODE_AUTH_TOKEN
  CARGO_REGISTRY_TOKEN    set: publish the crate; unset: skip it, as always
  GITHUB_TOKEN, GITHUB_REPOSITORY, GITHUB_API_URL   the Release's assets and notes
  ONEPIPELINE_PYPI_UPLOAD_URL, ONEPIPELINE_PYPI_INDEX_URL, ONEPIPELINE_PYPI_JSON_URL,
  ONEPIPELINE_CRATE_REGISTRY, npm_config_registry   where each registry is (default: the public one)
  ONEPIPELINE_RELEASE_VERIFY_BUDGET   seconds an install may wait for a registry (default 600)
  ONEPIPELINE_RELEASE_OUT where each platform's files go (default target/release-artifacts)

Exit 0: the phase completed. Exit 1: the release was refused or a step failed,
named on stderr with what to do next. Exit 2: the command line was refused.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path.cwd()
SCRIPTS = Path(__file__).resolve().parent

_spec = importlib.util.spec_from_file_location('staged', SCRIPTS / 'build-staged-release.py')
staged = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(staged)

# The targets each build job builds, once each. Intel macOS and Linux arm64 are
# not built: README.md says how to install there from source.
PLATFORMS = {
    'linux': ['x86_64-unknown-linux-gnu'],
    'macos': ['aarch64-apple-darwin'],
    'windows': ['x86_64-pc-windows-msvc'],
}

# The wheel platform tag each target's build is held to, and the glibc floor a
# Linux binary may not exceed. Linux builds once, inside the manylinux2014 image,
# so its archive and npm package carry the wheel's binary and the wheel's floor.
WHEEL_PLATFORM = {
    'x86_64-unknown-linux-gnu': 'manylinux_2_17_x86_64.manylinux2014_x86_64',
    'aarch64-apple-darwin': 'macosx_11_0_arm64',
    'x86_64-pc-windows-msvc': 'win_amd64',
}
GLIBC_FLOOR = {'x86_64-unknown-linux-gnu': (2, 17)}

PYPI_NAME = 'onepipeline-cli'
NPM_LAUNCHER = 'onepipeline-cli'
CRATE = 'onepipeline'
OUTCOME_FILE = 'release-outcome.json'
# What release-outcome.json answers for: release-targets.toml's [[target]] ids.
OUTCOME_TARGETS = [f'crate:{CRATE}', f'pypi:{PYPI_NAME}', f'npm:{NPM_LAUNCHER}']
# The line `linked-engines.sh --format notes` opens its section with.
LINKED_ENGINES_MARKER = '<!-- linked-engines -->'


class Refused(Exception):
    """A refusal: the message, and what to do about it."""

    def __init__(self, message, action):
        super().__init__(message)
        self.action = action


def say(message):
    print(f'release: {message}', flush=True)


def run(*args, env=None, cwd=None, quiet=False):
    """Run one command; a failure carries its output into the refusal.

    The program is resolved on PATH first, because Windows runs `npm` and `uv`
    only by their full `.cmd`/`.exe` path.
    """
    args = (shutil.which(str(args[0]), path=(env or os.environ).get('PATH')) or args[0], *args[1:])
    result = subprocess.run(args, cwd=cwd or ROOT, env=env, text=True,
                            stdout=subprocess.PIPE if quiet else None,
                            stderr=subprocess.STDOUT if quiet else None)
    if result.returncode != 0:
        if quiet and result.stdout:
            sys.stderr.write(result.stdout)
        raise Refused(f"'{' '.join(str(a) for a in args)}' exited {result.returncode}",
                      'fix what the output above names, then re-run this phase')
    return result.stdout or ''


# --- what a release is -------------------------------------------------------

def cargo_version():
    try:
        return staged.release_version()
    except (ValueError, OSError) as error:
        raise Refused(f'cannot read the release version from Cargo.toml: {error}',
                      "restore Cargo.toml's [package] version") from error


def release_tag(dry_run):
    tag = os.environ.get('RELEASE_TAG', '')
    if not tag:
        if not dry_run:
            raise Refused('RELEASE_TAG is not set, so nothing says which Release this is',
                          "set RELEASE_TAG to the Release's tag, as release.yml does, or pass --dry-run")
        return f'v{cargo_version()}'
    return tag


def tag_version(tag):
    """The version a release tag names, or a refusal: `v` and a SemVer, nothing else."""
    if not re.fullmatch('v' + staged.SEMVER_PATTERN, tag):
        raise Refused(f"release tag '{tag}' is not a vX.Y.Z version tag",
                      'cut the Release from a v<version> tag; release-plz names them so')
    return tag[1:]


def guard(phase, dry_run=False):
    """Whether this release may publish at all, and through which registries.

    The checks the release used to spread over a `guard` job and a tag check in
    every verify job: a tag that is not `v<semver>`, or names another version
    than Cargo.toml, is refused before any build or upload; so is a registry
    switched on with no credential to publish with. A missing crates.io token
    turns the crate off rather than refusing, as it always has.
    """
    tag = release_tag(dry_run)
    tag_version(tag)
    version = cargo_version()
    if tag != f'v{version}':
        raise Refused(f"release tag '{tag}' does not name Cargo.toml's version {version}",
                      'cut the Release from the commit whose Cargo.toml carries that version')
    channels = {
        'assets': True,
        'pypi': os.environ.get('PYPI_PUBLISH') == 'true',
        'npm': os.environ.get('NPM_PUBLISH') == 'true',
        'crate': bool(os.environ.get('CARGO_REGISTRY_TOKEN')),
    }
    if channels['pypi'] and not os.environ.get('PYPI_TOKEN'):
        raise Refused('PYPI_PUBLISH is on but PYPI_TOKEN is empty',
                      'add the PYPI_TOKEN secret (see gh-secrets.json) or unset PYPI_PUBLISH')
    if channels['npm'] and not os.environ.get('NODE_AUTH_TOKEN'):
        raise Refused('NPM_PUBLISH is on but NODE_AUTH_TOKEN is empty',
                      'add the NPM_TOKEN secret (see gh-secrets.json) or unset NPM_PUBLISH')
    if phase != 'build' and not dry_run:
        for name in ['GITHUB_TOKEN', 'GITHUB_REPOSITORY']:
            if not os.environ.get(name):
                raise Refused(f'{name} is not set, so the Release cannot be read or written',
                              f'set {name}, as every Actions job has it')
    if not channels['crate'] and phase == 'publish-common':
        say('CARGO_REGISTRY_TOKEN not set; skipping crates.io publish.')
    for name in ['pypi', 'npm']:
        if not channels[name] and phase != 'build':
            say(f'{name.upper()}_PUBLISH is not on; skipping {name}.')
    return tag, version, channels


def wheel_name(version, target):
    return f'onepipeline_cli-{version}-py3-none-{WHEEL_PLATFORM[target]}.whl'


def npm_package(target):
    facts = staged.target_facts()[target]
    return f"onepipeline-cli-{facts['platform']}-{facts['arch']}", facts


def check_targets():
    declared = set(staged.supported_targets())
    built = {target for targets in PLATFORMS.values() for target in targets}
    if declared != built:
        raise Refused(f'scripts/npm-build.mjs declares {sorted(declared)} but the build jobs build {sorted(built)}',
                      "keep npm-build.mjs's TARGETS and PLATFORMS in scripts/release.py the same set")


def plan(platform, version):
    """Every artifact one job publishes, named the way the registry serves it."""
    check_targets()
    artifacts = []
    if platform == 'common':
        artifacts.append({'kind': 'crate', 'target': None, 'file': f'{CRATE}-{version}.crate',
                          'package': f'crate:{CRATE}', 'destination': 'crates.io'})
        artifacts.append({'kind': 'npm', 'target': None, 'file': f'{NPM_LAUNCHER}-{version}.tgz',
                          'package': f'npm:{NPM_LAUNCHER}', 'destination': 'npm'})
        artifacts.append({'kind': 'outcome', 'target': None, 'file': OUTCOME_FILE,
                          'package': None, 'destination': 'github-release'})
        return {'platform': platform, 'version': version, 'targets': [], 'artifacts': artifacts}
    for target in PLATFORMS[platform]:
        floor = GLIBC_FLOOR.get(target)
        extra = {'glibc_floor': '.'.join(map(str, floor))} if floor else {}
        archive = staged.archive_name(version, target)
        artifacts.append({'kind': 'archive', 'target': target, 'file': archive,
                          'package': None, 'destination': 'github-release', **extra})
        artifacts.append({'kind': 'checksum', 'target': target, 'file': f'onepipeline-v{version}-{target}.sha256',
                          'package': None, 'destination': 'github-release'})
        artifacts.append({'kind': 'wheel', 'target': target, 'file': wheel_name(version, target),
                          'package': f'pypi:{PYPI_NAME}', 'destination': 'pypi',
                          'wheel_tags': [f'py3-none-{tag}' for tag in WHEEL_PLATFORM[target].split('.')], **extra})
        name, facts = npm_package(target)
        artifacts.append({'kind': 'npm', 'target': target, 'file': f'{name}-{version}.tgz',
                          'package': f'npm:{name}', 'destination': 'npm',
                          'os': facts['platform'], 'cpu': facts['arch'], **extra})
    return {'platform': platform, 'version': version, 'targets': PLATFORMS[platform], 'artifacts': artifacts}


# --- build -------------------------------------------------------------------

def out_dir(args):
    base = Path(os.environ.get('ONEPIPELINE_RELEASE_OUT') or ROOT / 'target' / 'release-artifacts')
    return (args.out or base / args.platform).resolve()


def fresh(out):
    """An output directory this script owns, emptied: a marker says it is ours."""
    marker = out / '.release-artifacts'
    if out.exists() and any(out.iterdir()):
        if not marker.exists():
            raise Refused(f'{out} is not empty and is not a directory this script made',
                          'pass --out an empty directory, or remove that one')
        shutil.rmtree(out)
    out.mkdir(parents=True, exist_ok=True)
    marker.write_text('scripts/release.py output\n')


def glibc_needed(binary):
    """The highest GLIBC_ symbol version a Linux binary requires."""
    if not shutil.which('objdump'):
        raise Refused('objdump not found, so the glibc floor cannot be read',
                      'install binutils, then re-run')
    symbols = subprocess.run(['objdump', '-T', str(binary)], text=True, capture_output=True, check=True).stdout
    versions = {tuple(int(part) for part in found) for found in re.findall(r'GLIBC_(\d+)\.(\d+)', symbols)}
    return max(versions) if versions else (0, 0)


def conform(record, version):
    """Hold one target's built files to its plan, so the plan is what ships."""
    target = record['target']
    wanted = {artifact['kind']: artifact for artifact in plan_for_target(target, version)}
    for kind, path in [('wheel', record['wheel']), ('archive', record['archive']), ('npm', record['npm'])]:
        if Path(path).name != wanted[kind]['file']:
            raise Refused(f'{target} built {Path(path).name}, but the plan names {wanted[kind]["file"]}',
                          'make the build produce the planned name, or change the plan and its test deliberately')
    if target in GLIBC_FLOOR:
        with zipfile.ZipFile(record['wheel']) as wheel:
            member = next(name for name in wheel.namelist() if name.endswith('.data/scripts/onepipeline'))
            with tempfile.TemporaryDirectory() as scratch:
                binary = Path(scratch) / 'onepipeline'
                binary.write_bytes(wheel.read(member))
                needed = glibc_needed(binary)
        if needed > GLIBC_FLOOR[target]:
            raise Refused(f'{target} requires GLIBC_{needed[0]}.{needed[1]}, above the planned floor '
                          f'{wanted["wheel"]["glibc_floor"]}', 'build it inside the manylinux2014 image, as scripts/build-linux-wheel.sh does')


def plan_for_target(target, version):
    platform = next(name for name, targets in PLATFORMS.items() if target in targets)
    return [artifact for artifact in plan(platform, version)['artifacts'] if artifact['target'] == target]


def compile_wheel(platform, target, out):
    if platform == 'linux':
        wheels = out / 'wheels' / target
        wheels.mkdir(parents=True, exist_ok=True)
        try:
            relative = wheels.relative_to(ROOT)
        except ValueError as error:
            raise Refused(f'{out} is outside the checkout, which the manylinux container cannot see',
                          'pass --out a directory inside the repository') from error
        run('bash', str(SCRIPTS / 'build-linux-wheel.sh'), '--target', target, '--out', str(relative))
        wheel, = wheels.glob('*.whl')
        return wheel
    os.environ['RUSTFLAGS'] = '-D warnings'
    return staged.compile_wheel(target, out, 'release')


def build(args):
    tag, version, _ = guard('build', dry_run=args.dry_run)
    out = out_dir(args)
    fresh(out)
    built = []
    for target in PLATFORMS[args.platform]:
        say(f'building {target} once')
        wheel = compile_wheel(args.platform, target, out)
        try:
            record = staged.package(target, wheel, out, version)
        except (ValueError, OSError, subprocess.SubprocessError) as error:
            raise Refused(f'{target} could not be packaged: {error}',
                          'fix what the packaging reports, then re-run the build') from error
        conform(record, version)
        say(f'test-installing the {target} wheel, npm package and archive')
        try:
            staged.install(record, out, sys.executable, shutil.which('node') or 'node')
        except subprocess.CalledProcessError as error:
            for output in [error.stdout, error.stderr]:
                if output:
                    sys.stderr.write(output)
            raise Refused(f'the {target} test-install failed, so nothing is published',
                          'fix what the install above reports; the release publishes only files that installed') from error
        built.append(record)
    files = []
    for record in built:
        for artifact in plan_for_target(record['target'], version):
            path = {'archive': Path(record['archive']),
                    'checksum': Path(record['archive']).with_name(artifact['file']),
                    'wheel': Path(record['wheel']), 'npm': Path(record['npm'])}[artifact['kind']]
            files.append({**artifact, 'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
    manifest = {'schema_version': 1, 'platform': args.platform, 'tag': tag, 'version': version, 'artifacts': files}
    (out / 'build.json').write_text(json.dumps(manifest, indent=2) + '\n')
    say(f'built, packaged and test-installed {len(built)} target(s) into {out}')


def built(out, version):
    try:
        manifest = json.loads((out / 'build.json').read_text())
    except (OSError, ValueError) as error:
        raise Refused(f'no build in {out}: {error}', 'run the build phase first, into the same --out') from error
    if not isinstance(manifest, dict) or manifest.get('version') != version:
        raise Refused(f'the build in {out} is not of version {version}', 'rebuild for this release')
    for artifact in manifest['artifacts']:
        path = Path(artifact['path'])
        if hashlib.sha256(path.read_bytes()).hexdigest() != artifact['sha256']:
            raise Refused(f'{path} changed after it was built and test-installed',
                          'rebuild; only the files the test-install ran may be published')
    return manifest


# --- the registries ----------------------------------------------------------

def http(method, url, body=None, headers=None, token=None):
    request = urllib.request.Request(url, data=body, method=method, headers=dict(headers or {}))
    if token:
        request.add_header('Authorization', f'Bearer {token}')
    request.add_header('User-Agent', 'onepipeline-release')
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()
    except (urllib.error.URLError, OSError) as error:
        raise Refused(f'{method} {url} did not answer: {error}',
                      're-run once the service answers') from error


class Release:
    """The GitHub Release this run publishes into."""

    def __init__(self, tag):
        self.api = os.environ.get('GITHUB_API_URL', 'https://api.github.com').rstrip('/')
        self.repo = os.environ['GITHUB_REPOSITORY']
        self.token = os.environ['GITHUB_TOKEN']
        self.tag = tag
        status, body = self.call('GET', f'/repos/{self.repo}/releases/tags/{urllib.parse.quote(tag)}')
        if status != 200:
            raise Refused(f'GitHub answered {status} for the Release tagged {tag}',
                          'create the Release first: release-plz does, or cut it by hand from the tag')
        self.data = json.loads(body)

    def call(self, method, path, body=None, headers=None):
        url = path if path.startswith('http') else self.api + path
        return http(method, url, body, {'Accept': 'application/vnd.github+json', **(headers or {})}, self.token)

    def assets(self):
        status, body = self.call('GET', f"/repos/{self.repo}/releases/{self.data['id']}/assets?per_page=100")
        if status != 200:
            raise Refused(f'GitHub answered {status} listing the Release assets', 're-run once GitHub answers')
        return {asset['name']: asset for asset in json.loads(body)}

    def upload(self, path, replace=False):
        existing = self.assets().get(path.name)
        if existing and not replace:
            say(f'{path.name} is already on the Release; not uploading it again')
            return False
        if existing:
            status, _ = self.call('DELETE', f"/repos/{self.repo}/releases/assets/{existing['id']}")
            if status not in (204, 404):
                raise Refused(f'GitHub answered {status} replacing {path.name}', 're-run once GitHub answers')
        target = re.sub(r'\{.*\}$', '', self.data['upload_url']) + '?name=' + urllib.parse.quote(path.name)
        status, body = self.call('POST', target, path.read_bytes(), {'Content-Type': 'application/octet-stream'})
        if status != 201:
            raise Refused(f'GitHub answered {status} uploading {path.name}: {body[:200]!r}',
                          're-run the job; a re-run uploads only what is missing')
        say(f'uploaded {path.name} to the Release')
        return True

    def download(self, asset):
        # The public download URL, without the token: GitHub redirects it to
        # storage that refuses a request carrying a second credential.
        status, body = http('GET', asset['browser_download_url'])
        if status != 200:
            raise Refused(f"GitHub answered {status} for {asset['name']}", 're-run once GitHub answers')
        return body

    def notes(self, section):
        """Record the linked engines in the Release body, replacing the section a
        previous run of this release recorded rather than stacking a second."""
        lines = (self.data.get('body') or '').split('\n')
        kept = '\n'.join(lines[:lines.index(LINKED_ENGINES_MARKER)] if LINKED_ENGINES_MARKER in lines else lines)
        status, _ = self.call('PATCH', f"/repos/{self.repo}/releases/{self.data['id']}",
                              json.dumps({'body': kept.rstrip('\n') + '\n\n' + section}).encode(),
                              {'Content-Type': 'application/json'})
        if status != 200:
            raise Refused(f'GitHub answered {status} recording the linked engines', 're-run once GitHub answers')


def pypi_urls():
    return (os.environ.get('ONEPIPELINE_PYPI_UPLOAD_URL', 'https://upload.pypi.org/legacy/'),
            os.environ.get('ONEPIPELINE_PYPI_INDEX_URL', 'https://pypi.org/simple/'),
            os.environ.get('ONEPIPELINE_PYPI_JSON_URL', 'https://pypi.org/pypi').rstrip('/'))


def pypi_files(version):
    """The filenames PyPI serves for this version; none when it has no release of it."""
    status, body = http('GET', f'{pypi_urls()[2]}/{PYPI_NAME}/{version}/json')
    if status == 404:
        return set()
    if status != 200:
        raise Refused(f'PyPI answered {status} for {PYPI_NAME} {version}', 're-run once PyPI answers')
    return {entry['filename'] for entry in json.loads(body).get('urls', [])}


def npm_serves(package, version):
    view = subprocess.run(['npm', 'view', f'{package}@{version}', 'version', '--prefer-online'],
                          text=True, capture_output=True)
    if view.returncode == 0:
        return view.stdout.strip() == version
    if 'E404' in view.stderr or '404 Not Found' in view.stderr:
        return False
    raise Refused(f'npm could not be asked about {package}@{version}: {view.stderr.strip()}',
                  're-run once the npm registry answers')


def crate_registry():
    return os.environ.get('ONEPIPELINE_CRATE_REGISTRY', '')


def index_path(name):
    """Where a sparse registry index files a crate: by its name's length and first letters."""
    name = name.lower()
    if len(name) <= 2:
        return f'{len(name)}/{name}'
    if len(name) == 3:
        return f'3/{name[0]}/{name}'
    return f'{name[:2]}/{name[2:4]}/{name}'


def crate_serves(version):
    index = (crate_registry() or 'https://index.crates.io').rstrip('/')
    status, body = http('GET', f'{index}/{index_path(CRATE)}')
    if status == 404:
        return False
    if status != 200:
        raise Refused(f'the crates.io index answered {status} for {CRATE}', 're-run once it answers')
    return any(json.loads(line).get('vers') == version for line in body.decode().splitlines() if line.strip())


def crate_args():
    return ['--index', f'sparse+{crate_registry()}'] if crate_registry() else []


# --- publish -----------------------------------------------------------------

def publish(args):
    tag, version, channels = guard('publish', dry_run=args.dry_run)
    out = out_dir(args)
    manifest = built(out, version)
    uploads = [artifact for artifact in manifest['artifacts']
               if artifact['destination'] == 'github-release' or channels.get(artifact['destination'])]
    if args.dry_run:
        for artifact in uploads:
            say(f"would publish {artifact['file']} to {artifact['destination']}")
        say(f'dry run: published nothing for {args.platform}')
        return
    release = Release(tag)
    for artifact in uploads:
        if artifact['destination'] == 'github-release':
            release.upload(Path(artifact['path']))
    wheels = [Path(a['path']) for a in uploads if a['destination'] == 'pypi']
    if wheels:
        upload, index, _ = pypi_urls()
        env = dict(os.environ, UV_PUBLISH_TOKEN=os.environ['PYPI_TOKEN'])
        # `--check-url` asks the index first, so a re-run uploads only the wheels
        # it does not already serve.
        run('uv', 'publish', '--trusted-publishing', 'never', '--publish-url', upload,
            '--check-url', index, *[str(wheel) for wheel in wheels], env=env)
    for artifact in uploads:
        if artifact['destination'] == 'npm':
            run('bash', str(SCRIPTS / 'publish-npm.sh'), artifact['path'])
    say(f'published {len(uploads)} {args.platform} artifact(s) for {version}')


def smoke(directory, version, label):
    env = dict(os.environ, PATH=f'{directory}{os.pathsep}{os.environ["PATH"]}')
    run('bash', str(SCRIPTS / 'smoke-published.sh'), '--expect-version', version, '--label', label, env=env)


def retry_install(label, action, *command):
    budget = os.environ.get('ONEPIPELINE_RELEASE_VERIFY_BUDGET', '600')
    run('bash', str(SCRIPTS / 'retry-install.sh'), '--budget', budget, '--label', label,
        '--action', action, '--', *command)


def verify(args):
    """Install what this platform published, from the registries, and run it."""
    tag, version, channels = guard('verify')
    release = Release(tag)
    assets = release.assets()
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        for target in PLATFORMS[args.platform]:
            archive = staged.archive_name(version, target)
            checksum = f'onepipeline-v{version}-{target}.sha256'
            for name in [archive, checksum]:
                if name not in assets:
                    raise Refused(f'the Release serves no {name}', 're-run the build job that publishes it')
            payload = release.download(assets[archive])
            recorded = release.download(assets[checksum]).decode().split()
            if not recorded or recorded[0] != hashlib.sha256(payload).hexdigest():
                raise Refused(f'{archive} on the Release does not match its {checksum}',
                              'delete both assets and re-run the build job')
            unpacked = scratch / 'archive' / target
            unpacked.mkdir(parents=True)
            (scratch / archive).write_bytes(payload)
            if archive.endswith('.zip'):
                with zipfile.ZipFile(scratch / archive) as bundle:
                    bundle.extractall(unpacked)
            else:
                with tarfile.open(scratch / archive) as bundle:
                    bundle.extractall(unpacked, filter='data')
            smoke(unpacked, version, f'Release archive {archive}')
            if channels['pypi']:
                venv = scratch / 'pypi' / target
                run(sys.executable, '-m', 'venv', str(venv))
                bindir = venv / ('Scripts' if os.name == 'nt' else 'bin')
                python = bindir / ('python.exe' if os.name == 'nt' else 'python')
                retry_install(f'PyPI {PYPI_NAME} {version} for {target}',
                              f'check {PYPI_NAME} {version} on the index: this job may have uploaded no wheel for {target}',
                              str(python), '-m', 'pip', 'install', '--no-cache-dir', '--index-url', pypi_urls()[1],
                              f'{PYPI_NAME}=={version}')
                smoke(bindir, version, f'PyPI {PYPI_NAME} {version} on {target}')
            if channels['npm']:
                name, _ = npm_package(target)
                prefix = scratch / 'npm' / target
                prefix.mkdir(parents=True)
                retry_install(f'npm {name} {version}', f'check {name}@{version} on the npm registry',
                              'npm', 'install', '--prefix', str(prefix), '--prefer-online', '--no-audit',
                              '--no-fund', f'{name}@{version}')
                smoke(prefix / 'node_modules' / name / 'bin', version, f'npm {name} {version}')
    say(f'every {args.platform} artifact of {version} installs from its registry and runs')


# --- publish: the platform-independent artifacts -----------------------------

def availability(version, channels, common=False):
    """What is missing from any registry for any platform, as one list."""
    missing = []
    release = Release(f'v{version}')
    assets = release.assets()
    pypi = pypi_files(version) if channels['pypi'] else set()
    for platform in PLATFORMS:
        for artifact in plan(platform, version)['artifacts']:
            where = artifact['destination']
            if where == 'github-release' and artifact['file'] not in assets:
                missing.append(f"{artifact['file']} on the Release")
            elif where == 'pypi' and channels['pypi'] and artifact['file'] not in pypi:
                missing.append(f"{artifact['file']} on PyPI")
            elif where == 'npm' and channels['npm'] and not npm_serves(artifact['package'].split(':', 1)[1], version):
                missing.append(f"{artifact['package']}@{version} on npm")
    if common:
        if channels['crate'] and not crate_serves(version):
            missing.append(f'crate:{CRATE}@{version} on crates.io')
        if channels['npm'] and not npm_serves(NPM_LAUNCHER, version):
            missing.append(f'npm:{NPM_LAUNCHER}@{version} on npm')
    return missing


def results_path(out):
    return out / 'publish-results.json'


def publish_common(args):
    args.platform = 'common'
    out = out_dir(args)
    out.mkdir(parents=True, exist_ok=True)
    results = {'crate': {'published': 'skipped', 'verified': 'skipped'},
               'npm-launcher': {'published': 'skipped', 'verified': 'skipped'}}
    results_path(out).unlink(missing_ok=True)

    def record():
        results_path(out).write_text(json.dumps(results, indent=2) + '\n')

    tag, version, channels = guard('publish-common', dry_run=args.dry_run)
    # A lock that links an engine twice is a build whose behaviour depends on
    # which copy each crate reaches; it is refused before anything is written.
    run('bash', str(SCRIPTS / 'linked-engines.sh'), '--format', 'split')
    if args.dry_run:
        for artifact in plan('common', version)['artifacts']:
            if artifact['kind'] == 'crate' and not channels['crate']:
                continue
            if artifact['kind'] == 'npm' and not channels['npm']:
                continue
            say(f"would publish {artifact['file']} to {artifact['destination']}")
        say('dry run: published nothing platform-independent')
        return
    record()
    missing = availability(version, channels)
    if missing:
        raise Refused('not every platform is published, so nothing platform-independent is: missing '
                      + '; '.join(missing), 're-run the build job that publishes what is missing, then this one')
    notes = run('bash', str(SCRIPTS / 'linked-engines.sh'), '--format', 'notes', quiet=True)
    Release(tag).notes(notes)
    if channels['crate']:
        results['crate']['published'] = 'failure'
        record()
        if crate_serves(version):
            say(f'{CRATE} {version} is already on crates.io; nothing to publish.')
        else:
            env = dict(os.environ, RUSTFLAGS='-D warnings')
            # crates.io takes the token from CARGO_REGISTRY_TOKEN; a registry
            # named by its index is handed it explicitly.
            token = ['--token', os.environ['CARGO_REGISTRY_TOKEN']] if crate_registry() else []
            run('cargo', 'publish', '--locked', *crate_args(), *token, env=env)
        results['crate']['published'] = 'success'
    if channels['npm']:
        results['npm-launcher']['published'] = 'failure'
        record()
        launcher = run('node', str(SCRIPTS / 'npm-build.mjs'), 'launcher', '--out', str(out / 'npm'), quiet=True).strip()
        run('bash', str(SCRIPTS / 'publish-npm.sh'), launcher)
        results['npm-launcher']['published'] = 'success'
    record()
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        if channels['crate']:
            results['crate']['verified'] = 'failure'
            record()
            retry_install(f'crates.io {CRATE} {version}', f'check {CRATE} {version} on crates.io',
                          'cargo', 'install', CRATE, '--version', version, '--locked', '--force',
                          '--root', str(scratch / 'cargo'), *crate_args())
            smoke(scratch / 'cargo' / 'bin', version, f'crates.io {CRATE} {version}')
            results['crate']['verified'] = 'success'
        if channels['npm']:
            results['npm-launcher']['verified'] = 'failure'
            record()
            prefix = scratch / 'npm'
            retry_install(f'npm {NPM_LAUNCHER} {version}', f'check {NPM_LAUNCHER}@{version} on the npm registry',
                          'npm', 'install', '--global', '--prefix', str(prefix), '--prefer-online', '--no-audit',
                          '--no-fund', f'{NPM_LAUNCHER}@{version}')
            smoke(prefix / ('' if os.name == 'nt' else 'bin'), version, f'npm {NPM_LAUNCHER} {version}')
            results['npm-launcher']['verified'] = 'success'
    record()
    missing = availability(version, channels, common=True)
    if missing:
        raise Refused('a registry does not serve every artifact of this release: missing ' + '; '.join(missing),
                      're-run the job that publishes what is missing')
    say(f'every registry serves {version} for every platform')


# --- the outcome -------------------------------------------------------------

RESULTS = ['success', 'failure', 'cancelled', 'skipped']


def registry_outcome(published, verified):
    """One registry's (published, verified) over every job that published into it.

    Every job published: the worst verification. Some but not all: the release
    shipped that registry partially, which is shipped and not verified. None: the
    worst publication, and no verification of what was never shipped.
    """
    for result in [*published, *verified]:
        if result not in RESULTS:
            raise Refused(f"'{result}' is not a job or step result GitHub reports",
                          'pass success, failure, cancelled or skipped')
    worst = lambda results: next((r for r in ['failure', 'cancelled', 'skipped'] if r in results), 'success')
    if all(result == 'success' for result in published):
        return 'success', worst(verified)
    if 'success' in published:
        return 'success', 'failure'
    return worst(published), 'skipped'


def outcome(args):
    """Compose release-outcome.json from what every job did, and attach it.

    A build job reports its publish and verify steps' outcomes; one that never
    reached them reports its job result for both. `publish` reports the crate
    and the launcher through the results file its publish step wrote.
    """
    tag = release_tag(dry_run=False)
    version = tag_version(tag)
    for name in ['GITHUB_TOKEN', 'GITHUB_REPOSITORY']:
        if not os.environ.get(name):
            raise Refused(f'{name} is not set, so the outcome cannot be attached to the Release',
                          f'set {name}, as every Actions job has it')
    out = out_dir(argparse.Namespace(out=args.out, platform='common'))
    out.mkdir(parents=True, exist_ok=True)
    try:
        results = json.loads(results_path(out).read_text())
    except (OSError, ValueError):
        results = {'crate': {'published': 'skipped', 'verified': 'skipped'},
                   'npm-launcher': {'published': 'skipped', 'verified': 'skipped'}}
    builds = []
    for platform in PLATFORMS:
        key = platform.upper()
        job = os.environ.get(f'{key}_RESULT') or 'skipped'
        builds.append({phase: os.environ.get(f'{key}_{phase.upper()}') or job for phase in ['published', 'verified']})
    crate, pypi, npm = OUTCOME_TARGETS
    targets = {crate: (results['crate']['published'], results['crate']['verified'])}
    targets[pypi] = (registry_outcome([b['published'] for b in builds], [b['verified'] for b in builds])
                                    if os.environ.get('PYPI_PUBLISH') == 'true' else ('skipped', 'skipped'))
    launcher = results['npm-launcher']
    targets[npm] = (registry_outcome([b['published'] for b in builds] + [launcher['published']],
                                                       [b['verified'] for b in builds] + [launcher['verified']])
                                      if os.environ.get('NPM_PUBLISH') == 'true' else ('skipped', 'skipped'))
    document = out / OUTCOME_FILE
    run_url = (f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{os.environ['GITHUB_REPOSITORY']}"
               f"/actions/runs/{os.environ.get('GITHUB_RUN_ID', '0')}")
    arguments = []
    for target, (published, verified) in targets.items():
        arguments += ['--target', target, '--published', published, '--verified', verified]
    run('node', str(SCRIPTS / 'release-outcome.mjs'), '--out', str(document), '--version', version,
        '--run-url', run_url, *arguments)
    # Replaced on a re-run, because a re-run is how a release moves out of
    # `shipped-unverified`, and the record has to move with it.
    Release(tag).upload(document, replace=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    commands = parser.add_subparsers(dest='command', required=True)
    planning = commands.add_parser('plan')
    planning.add_argument('platform', choices=[*PLATFORMS, 'common'])
    planning.add_argument('--version')
    for name in ['build', 'publish', 'verify', 'dry-run']:
        phase = commands.add_parser(name)
        phase.add_argument('platform', choices=list(PLATFORMS))
        phase.add_argument('--out', type=Path)
        if name == 'publish':
            phase.add_argument('--dry-run', action='store_true')
    common = commands.add_parser('publish-common')
    common.add_argument('--out', type=Path)
    common.add_argument('--dry-run', action='store_true')
    recording = commands.add_parser('outcome')
    recording.add_argument('--out', type=Path)
    args = parser.parse_args()
    try:
        if args.command == 'plan':
            version = args.version or cargo_version()
            try:
                staged.validate_version(version)
            except ValueError as error:
                raise Refused(f"'{version}' is not a version", 'pass --version X.Y.Z') from error
            print(json.dumps(plan(args.platform, version), indent=2))
        elif args.command == 'build':
            args.dry_run = False
            build(args)
        elif args.command == 'publish':
            publish(args)
        elif args.command == 'verify':
            verify(args)
        elif args.command == 'dry-run':
            args.dry_run = True
            build(args)
            publish(args)
        elif args.command == 'publish-common':
            publish_common(args)
        else:
            outcome(args)
    except Refused as refusal:
        print(f'release: {refusal}', file=sys.stderr)
        print(f'ACTION: {refusal.action}', file=sys.stderr)
        raise SystemExit(1)


if __name__ == '__main__':
    main()
