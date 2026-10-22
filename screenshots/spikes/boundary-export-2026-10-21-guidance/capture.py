#!/usr/bin/env python3
"""Throwaway visual mock only; implements nothing.

Before is the real, unchanged `onepipeline boundary guidance` from this tree's
release binary; after is hardcoded proposed copy under the task-owned visibility
ruling. Recapture both:
    cargo build --release --bin onepipeline && python3 screenshots/spikes/boundary-export-2026-10-21-guidance/capture.py
"""
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
# Proposed copy. Every identity below is synthetic.
GUIDANCE = '''Private work reaches a public task through a screened export inside one coordinating plan.

Visibility belongs to each task. A private coordinating plan may contain public tasks; the plan itself sets no visibility for them.

1. Coordinate privately. One private plan holds both tasks:
     generic-example-producer   visibility: private   publish: "preserve"
     generic-example-consumer   visibility: public
2. Generalize into one neutral directory on the private producer. The engine preserves its branch rather than landing it.
3. Export only that directory as one neutral commit on the public base, carrying no private history, name, path or author.
4. The public consumer depends on the producer through that export, and its session starts from the screened commit.

Board: the canonical board home stays private and keeps the full graph, including the export dependency
  generic-example-producer -> generic-example-consumer.
Public projections are neutral: a public task's projection omits every private dependency and backlink.

Public projection (synthetic)
  task: generic-example-consumer
  title: Generic example refresh
  visibility: public
  dependencies: []
  backlink: none

Screening stays on: the export screens files and every commit subject and message. A change outside the source directory refuses the export. Keep the producer's opaque export branch name unchanged.
'''


def main():
    scratch = Path(os.environ['ONEPIPELINE_NODE_SCRATCH_DIR'])
    os.environ['TMPDIR'] = str(scratch)
    tools = scratch / 'visual-tools'
    tools.mkdir(exist_ok=True)
    subprocess.run(['bash', str(ROOT / 'screenshots/install-freeze.sh'), str(tools)], check=True)
    os.environ['PATH'] = str(tools) + os.pathsep + os.environ['PATH']
    world.clear_inherited_engine_and_git_settings()
    with tempfile.TemporaryDirectory(dir=scratch, prefix='boundary-guidance-') as directory:
        scene = world.World(Path(directory) / 'world', ROOT / 'target/release')
        scene.build()
        done = scene.cli('boundary', 'guidance')
        world.expect(done, 0, 'boundary guidance')
        versions = ('$ onepipeline boundary guidance\n' + done.stdout,
                    '$ onepipeline boundary guidance\n' + GUIDANCE)
        texts = []
        for raw in versions:
            clean = normalise.Normaliser({str(scene.root): '/home/you/onepipeline'}).text(raw)
            texts.append('\n'.join(part for line in clean.splitlines()
                                   for part in (textwrap.wrap(line, capture.WRAP_COLUMNS,
                                                             replace_whitespace=False) or [''])))
        # Equal terminal rows and the renderer's fixed width give matching dimensions.
        rows = max(text.count('\n') + 1 for text in texts)
        for version, text in zip(('before', 'after'), texts):
            capture.render(text + '\n' * (rows - text.count('\n')), HERE / f'guidance-{version}.png')


if __name__ == '__main__':
    main()
