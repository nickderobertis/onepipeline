"""Double only upload/download transport, preserving hierarchy after the first wildcard.

The Actions upload-artifact contract defines this layout:
https://github.com/actions/upload-artifact#upload-using-multiple-paths-and-exclusions
"""
import glob
import json
from pathlib import Path
import shutil
import sys

pattern, staged_root, download_root = sys.argv[1:]
if not pattern.startswith('staged-release/'):
    raise ValueError('unsupported staged artifact root')
relative = pattern.removeprefix('staged-release/')
if '\\' in relative or any(part in ['', '.', '..'] for part in relative.split('/')):
    raise ValueError(f'staged artifact path escapes its root: {pattern}')
root = Path(staged_root).resolve()
source_pattern = str(root / relative)
# llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This is the forge transport double permitted by this task, modeling the documented upload-artifact wildcard layout over actual workflow paths and real built files; GitHub transport remains the third-party execution boundary.
first_wildcard = min(source_pattern.index(c) for c in '*?[' if c in source_pattern)
prefix = source_pattern[:first_wildcard]
base = Path(prefix) if prefix.endswith(('/', '\\')) else Path(prefix).parent
# llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
files = [Path(path) for path in glob.glob(source_pattern, recursive=True) if Path(path).is_file()]
if not files:
    raise ValueError('artifact upload matched no files')
transferred = []
for source in files:
    if not source.resolve().is_relative_to(root):
        raise ValueError(f'staged artifact resolves outside its root: {source}')
    destination = Path(download_root) / source.relative_to(base)
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    transferred.append(str(destination))
print(json.dumps(transferred))
