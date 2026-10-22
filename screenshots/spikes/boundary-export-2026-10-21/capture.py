#!/usr/bin/env python3
"""Throwaway copy proposal only. Recapture: python3 screenshots/spikes/boundary-export-2026-10-21/capture.py"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'screenshots'))
import capture
import normalise
import world

HERE = Path(__file__).resolve().parent
GUIDANCE = '''Private work reaches a public node through a screened export.

1. Coordinate privately. Keep the producer and export coordination in a private plan.
2. Require mode: independent on the export. The public consumer has no dependency on the private producer.
3. Keep the canonical board home private. Publish only neutral public projections; a public view has no private backlink, private identifier, or private dependency.
4. Generalize into one neutral directory; preserve the producer branch. Export only that directory as a neutral commit on the public base, without private history, name, path, or author.
5. Consume the screened commit independently in the public plan.

Screening stays on: files and commit subjects/messages are screened. Changes outside the source directory refuse export. Keep the opaque export branch name unchanged.

Public projection (synthetic)
  title: Generic example refresh
  mode: independent
  dependencies: []
  backlink: none
  screening: on
'''
REFUSAL = '''$ onepipeline plan check demo:example
error: export mode is required; no export was started.

Repair: set mode: independent on the export in its coordinating plan, then retry.
Public work must remain independently runnable. Do not add a producer dependency or backlink to the public view.
Screening remains on; this repair does not bypass screening.
'''


def main():
    scratch = Path(os.environ['ONEPIPELINE_NODE_SCRATCH_DIR'])
    os.environ['TMPDIR'] = str(scratch)
    tools = scratch / 'visual-tools'
    tools.mkdir(exist_ok=True)
    subprocess.run(['bash', str(ROOT / 'screenshots/install-freeze.sh'), str(tools)], check=True)
    os.environ['PATH'] = str(tools) + os.pathsep + os.environ['PATH']
    world.clear_inherited_engine_and_git_settings()
    with tempfile.TemporaryDirectory(dir=scratch, prefix='boundary-capture-') as directory:
        scene = world.World(Path(directory) / 'world', ROOT / 'target/release')
        scene.build()
        done = scene.cli('boundary', 'guidance')
        world.expect(done, 0, 'boundary guidance')
        guidance = '$ onepipeline boundary guidance\n' + done.stdout
        # The old binary has no missing-mode refusal. This is a labeled scenario,
        # not a claimed capture of validation behavior that does not exist yet.
        old_refusal = ('$ onepipeline plan check demo:example\n'
                       '[Before: representative repair fixture; no missing-mode rule]\n'
                       'Repair advice available today:\n\n' + done.stdout)
        pairs = {
            'guidance': (guidance, '$ onepipeline boundary guidance\n' + GUIDANCE),
            'missing-mode': (old_refusal, REFUSAL),
        }
        for name, versions in pairs.items():
            texts = []
            for raw in versions:
                clean = normalise.Normaliser({str(scene.root): '/home/you/onepipeline'}).text(raw)
                texts.append('\n'.join(part for line in clean.splitlines()
                                       for part in (textwrap.wrap(line, capture.WRAP_COLUMNS,
                                                                 replace_whitespace=False) or [''])))
            # Equal terminal rows and the renderer's fixed width give matching dimensions.
            rows = max(text.count('\n') + 1 for text in texts)
            for version, text in zip(('before', 'after'), texts):
                padded = text + '\n' * (rows - text.count('\n'))
                capture.render(padded, HERE / f'{name}-{version}.png')


if __name__ == '__main__':
    main()
