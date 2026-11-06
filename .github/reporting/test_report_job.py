"""This repository's `budgets` job, through the released action, and the telemetry it records.

Each journey reads the job out of `ci.yml` and runs what its `uses:` pins with its `with:` inputs:
action.yml's own steps, the released driver and plugin archive, the published core wheel, the real
`gh`, onevcs and `actions/upload-artifact` code. Only GitHub's API and artifact services, the release
host and the package index are doubled, by the harness the action ships at that same tag. The
checkout holds this repository's real budgets files and the commands they name, on a runner's PATH.

The workload recorded for `onepipeline-budget-report-command-seconds`: 100 catalog rows (the real
catalog, filled with explicitly synthetic ones) and one rolling file of 20 commits, with 12 synthetic
REST-reading commands and a retried rate limit. It asserts behaviour and never a cost: the figures
are written to `.telemetry/report-job.json` and printed.
"""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import urllib.request
import zipfile

import yaml

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
ACTION = 'nickderobertis/onebudgetspec-github-action'
OWN_BUDGET = 'onepipeline-budget-report-command-seconds'
# Directories the action's discovery never treats as production budgets.
EXCLUDED_TREES = {'fixture', 'fixtures', '__fixtures__', 'testdata', 'conformance'}


def tracked():
    return subprocess.run(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT,
                          capture_output=True, text=True, check=True).stdout.split('\0')


# The action's own discovery: every file its `**/budgets.yaml` default matches, as `git` lists them.
CATALOG_FILES = tuple(sorted(name for name in tracked() if Path(name).name == 'budgets.yaml'
                             and not EXCLUDED_TREES & set(Path(name).parts) and (ROOT / name).is_file()))
# The checkout a journey's commands see: every budgets file, the code its commands name, and the
# two ignore files that keep a job's own outputs out of discovery.
CHECKOUT_PREFIXES = ('scripts/', *(f'{Path(name).parent}/' for name in CATALOG_FILES
                                   if not name.startswith('.github/') and '/' in name))
CHECKOUT_FILES = ('justfile', '.gitignore', '.github/.gitignore', '.github/macos-budget.mjs',
                  '.github/release-published-paths', '.github/telemetry.mjs',
                  '.github/workflow-graph.mjs', '.github/workflow-schema.mjs', *CATALOG_FILES)
ROWS = 100
COMMANDS_READING_REST = 12
COMMITS = 20
PUSH_EVENT = {'head_commit': {'message': 'feat: a change'}}
PREDECESSORS = ('lint', 'test-linux', 'test-windows', 'test-macos')
REQUESTS_METRIC = 'report-github-api-requests-per-run'
SECONDS_METRIC = 'report-command-seconds'
DESIGN_REQUESTS = 20
TELEMETRY = HERE / '.telemetry' / 'report-job.json'
TRANSFER_SERVICES = ('artifact-service', 'blob-storage', 'object-storage', 'release-host', 'package-index')
WRITE = 'echo "{\\"value\\": %s}" > "$ONEBUDGETSPEC_RESULT"'


def workflow():
    return yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())


def job_definition():
    return workflow()['jobs']['budgets']


def action_step():
    steps = [step for step in job_definition()['steps'] if step.get('uses', '').startswith(f'{ACTION}@')]
    assert len(steps) == 1, steps
    return steps[0]


def released_action():
    """The action at the workflow's pin: its tree, its harness and its real payload."""
    tag = action_step()['uses'].split('@', 1)[1]
    assert re.fullmatch(r'v\d+\.\d+\.\d+', tag), f'the job pins no exact release: {tag}'
    version = tag.removeprefix('v')
    directory = ROOT / 'target' / 'report-job' / f'action-{tag}'
    target = {('Linux', 'x86_64'): 'x86_64-unknown-linux-gnu', ('Linux', 'aarch64'): 'aarch64-unknown-linux-gnu',
              ('Darwin', 'arm64'): 'aarch64-apple-darwin'}[(platform.system(), platform.machine())]
    archive_name = f'onebudgetspec-github-action-{version}-{target}.tar.gz'
    saved = directory / 'dist' / archive_name
    if not saved.is_file():
        shutil.rmtree(directory, ignore_errors=True)
        directory.mkdir(parents=True)
        source = fetch(f'https://codeload.github.com/{ACTION}/tar.gz/refs/tags/{tag}')
        with tarfile.open(fileobj=io.BytesIO(source)) as archive:
            for member in archive.getmembers():
                member.name = member.name.split('/', 1)[-1]
                archive.extract(member, directory, filter='data')
        releases = f'https://github.com/{ACTION}/releases/download/{tag}'
        payload = fetch(f'{releases}/{archive_name}')
        (directory / 'dist').mkdir()
        (directory / 'dist' / 'SHA256SUMS').write_bytes(fetch(f'{releases}/SHA256SUMS'))
        (directory / 'dist' / 'action.yml').write_bytes(fetch(f'{releases}/action.yml'))
        listed = f'{hashlib.sha256(payload).hexdigest()}  {archive_name}'
        assert listed in (directory / 'dist' / 'SHA256SUMS').read_text().splitlines(), listed
        saved.write_bytes(payload)
    spec = importlib.util.spec_from_file_location('released_action_harness', directory / 'tests/action-harness/harness.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    # The harness builds this archive from source for the action's own tests; a consumer gets the released one.
    module._cached['archive'] = saved
    return tag, directory, module


def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as answer:
        return answer.read()


TAG, ACTION_TREE, harness = released_action()


def runner_path(directory):
    """What a hosted runner has for a budget command: no just, uv, cargo, bun or onevcs."""
    node = directory / 'node'
    node.symlink_to(shutil.which('node'))
    path = f'{directory}{os.pathsep}/usr/bin{os.pathsep}/bin'
    for tool in ('just', 'onevcs', 'cargo'):
        found = shutil.which(tool, path=path)
        assert found is None, f'{found} stands where a hosted runner has no {tool}'
    return path


def catalog():
    budgets = {}
    for name in CATALOG_FILES:
        for budget in yaml.safe_load((ROOT / name).read_text())['budgets']:
            budgets[budget['id']] = {**budget, 'file': name}
    return budgets


def checkout_files():
    wanted = [name for name in tracked() if name.startswith(CHECKOUT_PREFIXES)]
    return sorted({*wanted, *CHECKOUT_FILES})


def counting(then):
    return ['bash', '-c', f'echo "$ONEBUDGETSPEC_BUDGET_ID" >> "$JOURNEY_COUNTS"; {then}']


def mixed_catalog():
    """Synthetic: runnable budgets of different units and commands, most without the job's label."""
    return harness.budgets_yaml([
        ('synthetic-bundle-bytes', counting('echo "{\\"value\\": $(cat bundle-size)}" > "$ONEBUDGETSPEC_RESULT"'),
         {'unit': 'bytes', 'threshold': 1000}),
        ('synthetic-coverage-percent', ['node', 'measure.mjs'], {'unit': 'percent', 'direction': 'min', 'threshold': 80}),
        ('synthetic-host-reads', counting('gh api repos/owner/repo/commits/main > /dev/null; ' + WRITE % 1),
         {'unit': 'requests', 'threshold': 5, 'labels': ['ci-report']}),
        ('synthetic-walltime', counting('true'), {'unit': 'seconds', 'measure': 'elapsed', 'threshold': 60}),
        ('synthetic-release-figure', counting(WRITE % 50), {'unit': 'count', 'threshold': 10, 'labels': ['release-tail']}),
        ('synthetic-needs-a-tool', ['tool-this-runner-lacks', 'run'], {'labels': ['ci-report']}),
        ('synthetic-needs-telemetry', ['node', 'measure.mjs', 'dist/telemetry/journey.json'], {}),
    ])


MEASURE = """\
import { appendFileSync, writeFileSync } from 'node:fs';
appendFileSync(process.env.JOURNEY_COUNTS, `${process.env.ONEBUDGETSPEC_BUDGET_ID}\\n`);
writeFileSync(process.env.ONEBUDGETSPEC_RESULT, JSON.stringify({ value: 91, detail: 'lines' }));
"""
MIXED_RUNNABLE = sorted(['synthetic-bundle-bytes', 'synthetic-coverage-percent', 'synthetic-host-reads',
                         'synthetic-release-figure', 'synthetic-walltime'])


def fill_catalog(present):
    """Synthetic rows up to the workload's 100, 12 of them reading REST as the design allows."""
    entries = []
    for index in range(COMMANDS_READING_REST):
        # One reads through curl: the job cannot see it, and the boundary still counts it.
        read = (f'curl -fsS -H "Authorization: Bearer $GH_TOKEN" http://api.github.localhost/repos/owner/repo/commits/{index}'
                if index == 0 else f'gh api repos/owner/repo/commits/{index}')
        entries.append((f'synthetic-reads-rest-{index}', ['bash', '-c', f'{read} > /dev/null || exit 1; {WRITE % 1}'],
                        {'unit': 'requests'}))
    while len(entries) + present < ROWS:
        entries.append((f'synthetic-local-{len(entries)}', ['bash', '-c', WRITE % 1], {'unit': 'bytes'}))
    return harness.budgets_yaml(entries)


def schedule(workflow_file, event_name, results):
    """Whether the workflow file starts `budgets` for this event and these predecessor outcomes."""
    if event_name == 'pull_request':
        event = {'action': 'synchronize', 'repository': {'full_name': 'nickderobertis/onepipeline'}, 'pull_request': {
            'number': 1, 'draft': False, 'title': 'feat: a change',
            'head': {'ref': 'change', 'repo': {'full_name': 'nickderobertis/onepipeline'}}, 'base': {'ref': 'main'}}}
        ref = 'refs/pull/1/merge'
    else:
        event = PUSH_EVENT
        ref = 'refs/heads/main'
    scenario = {'eventName': event_name, 'ref': ref, 'head': '0' * 40, 'event': event, 'results': results}
    answered = subprocess.run(['node', str(HERE / 'ci-schedule.mjs'), str(workflow_file), json.dumps(scenario)], cwd=ROOT,
                              capture_output=True, text=True, check=True)
    return json.loads(answered.stdout)


def zip_of(name, content):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w') as archive:
        archive.writestr(name, content)
    return buffer.getvalue()


class Journey(unittest.TestCase):
    def setUp(self):
        self.host = harness.Host()
        self.addCleanup(self.host.close)
        self.repository = harness.Repository()
        self.addCleanup(self.repository.close)
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        (self.scratch / 'bin').mkdir()
        self.runner = runner_path(self.scratch / 'bin')
        self.counts = self.scratch / 'counts'
        self.next_run = 1000
        self.base_sha = None

    def write_real_catalog(self):
        for name in checkout_files():
            source = ROOT / name
            if source.is_file():
                (self.repository.path / name).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, self.repository.path / name)

    def write_mixed_catalog(self, bundle_size=500):
        self.repository.write('packages/demo/budgets.yaml', mixed_catalog())
        self.repository.write('packages/demo/measure.mjs', MEASURE)
        self.repository.write('packages/demo/bundle-size', f'{bundle_size}\n')

    def inputs(self):
        """The `with:` the committed job really names, its token expression replaced by the double's."""
        given = dict(action_step()['with'])
        self.assertEqual(given.pop('token'), '${{ github.token }}')
        return {'token': 'job-token', **given}

    def run_job(self, event_name, sha, **options):
        self.next_run += 1
        self.counts.unlink(missing_ok=True)
        environment = {'JOURNEY_COUNTS': str(self.counts), 'PATH': self.runner,
                       'GITHUB_REPOSITORY': harness.REPOSITORY, 'GITHUB_RUN_ID': str(self.next_run),
                       'RUNNER_NAME': 'hosted-runner'}
        job = harness.Job(self.host, self.repository, event_name, sha, inputs=self.inputs(), run_id=self.next_run,
                          environment=environment, action_path=ACTION_TREE, **options)
        self.addCleanup(job.close)
        return job.run()

    def base_push(self):
        self.repository.git('checkout', '--quiet', 'main')
        self.base_sha = self.repository.commit('merged')
        return self.run_job('push', self.base_sha)

    def pull_request(self, **event):
        self.repository.git('checkout', '--quiet', '-B', 'change', self.base_sha)
        return self.pull_request_head(**event)

    def pull_request_head(self, **event):
        head = self.repository.commit('change')
        self.host.head = head
        return self.run_job('pull_request', head, event=harness.event_pull_request(1, head, self.base_sha, **event),
                            base_ref='main', register=False)

    def executed(self):
        return sorted(self.counts.read_text().split()) if self.counts.exists() else []

    def budget_run(self, job):
        return job.saved('budget-run.json')

    def verdicts(self, job):
        return {result['id']: result for result in self.budget_run(job)['results']['results']}

    def unavailable(self, job):
        return {entry['id']: entry['reason'] for entry in self.budget_run(job)['unavailable']}


class RealCatalog(Journey):
    def setUp(self):
        super().setUp()
        self.write_real_catalog()
        self.base_sha = self.repository.commit('the real catalog')

    def assert_every_row_stated(self, job):
        budgets = catalog()
        self.assertEqual(job.exit_code, 0, job.stdout)
        self.assertEqual(sorted(job.rows()), sorted(budgets))
        results = self.verdicts(job)
        self.assertEqual([id for id, result in results.items() if result['verdict'] == 'error'], [],
                         'a budget reads as an error because its input is absent')
        own = results.pop(OWN_BUDGET)
        self.assertIn(own['verdict'], ('within', 'over'))
        self.assertEqual(results, {}, 'nothing else can measure in a checkout-only job')
        reasons = self.unavailable(job)
        self.assertEqual(sorted(reasons), sorted(set(budgets) - {OWN_BUDGET}))
        return budgets, reasons

    def test_a_pull_request_states_every_production_budget_and_why_it_cannot_run_here(self):
        job = self.pull_request()
        budgets, reasons = self.assert_every_row_stated(job)
        for budget, reason in reasons.items():
            command = budgets[budget]['command']
            if command[0] == 'just':
                self.assertIn('needs just', reason, budget)
            elif 'release-tail' in budgets[budget].get('labels', []):
                # cache-use reads GitHub itself rather than through onevcs, and refuses with no release to read.
                self.assertIn('no release' if 'cache-use' in command else 'onevcs', reason, budget)
            elif 'host-variable' in budgets[budget].get('labels', []):
                self.assertIn('report-only', reason, budget)
            else:
                named = [argument for argument in command[1:] if '/' in argument and ' ' not in argument][-1]
                self.assertIn(f'reads {named}, which is not available', reason, budget)
        # Nothing was built, run or recorded to fill a row, and no budget command spent a read.
        self.assertFalse((self.repository.path / 'target').exists())
        self.assertEqual(self.host.count('/releases|/runs/\\d+/jobs|/compare'), 0)
        self.assertEqual(len(self.host.comments), 1)
        for budget in budgets:
            self.assertIn(budget, self.host.comments[0]['body'])
        self.assertEqual(self.budget_run(job)['enforcement_exclude_labels'], ['release-tail', 'host-variable'])
        # The job's own budget ran after the main upload, and its completed verdict was saved.
        self.assertEqual(job.outcomes['main-upload'], 'success', job.stdout)
        self.assertEqual(job.outcomes['final-upload'], 'success', job.stdout)
        saved = json.loads(self.host.artifact_files(job.run_id)['budget-run.json'].decode())
        own = {result['id']: result for result in saved['results']['results']}[OWN_BUDGET]
        self.assertEqual(own['unit'], 'seconds')
        self.assertIsNotNone(self.host.artifact_files(job.run_id, 'budget-results-main'))

    def test_a_base_push_saves_the_same_results_and_posts_nothing(self):
        pull = self.pull_request()
        posted = len(self.host.comments)
        base = self.base_push()
        self.assert_every_row_stated(base)
        self.assertEqual(len(self.host.comments), posted)
        base_files = self.host.artifact_files(base.run_id)
        pull_files = self.host.artifact_files(pull.run_id)
        self.assertEqual(set(base_files), set(pull_files))
        # v0.1.1 decides on the rolling file before the own command runs, so a catalog with no other
        # runnable command starts no history, and says so. MixedCatalog proves history and comparison.
        self.assertIn('no rolling file saved', ' '.join(self.budget_run(base).get('diagnostics') or [])
                      + base.markdown() + base.summary)
        saved = json.loads(base_files['budget-run.json'].decode())
        self.assertEqual([result['id'] for result in saved['results']['results']], [OWN_BUDGET])

    def link_node_modules(self):
        (self.repository.path / 'node_modules').symlink_to(ROOT / 'node_modules')

    def record(self, wall_ms):
        directory = self.repository.path / 'target/budget-records'
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'stacked-spikes-stage.json').write_text(wall_ms)

    def test_a_budget_whose_input_is_present_keeps_its_verdict_and_enforcement(self):
        self.link_node_modules()
        self.record('{"wall_ms": 1000, "run_id": "run"}')
        job = self.pull_request()
        self.assertEqual(job.exit_code, 0, job.stdout)
        self.assertEqual(self.verdicts(job)['stacked-spikes-stage-time']['verdict'], 'within')

        self.record('{"wall_ms": 600000, "run_id": "run"}')
        job = self.pull_request()
        self.assertEqual(job.exit_code, 1, job.stdout)
        self.assertEqual(self.verdicts(job)['stacked-spikes-stage-time']['verdict'], 'over')

        self.record('not json')
        job = self.pull_request()
        self.assertEqual(job.exit_code, 3, 'a command that really breaks stays an error')
        self.assertEqual(self.verdicts(job)['stacked-spikes-stage-time']['verdict'], 'error')


class MixedCatalog(Journey):
    def setUp(self):
        super().setUp()
        self.write_mixed_catalog()
        self.base_sha = self.repository.commit('a mixed catalog')

    def test_every_runnable_budget_runs_once_and_only_the_unrunnable_ones_say_why(self):
        job = self.pull_request()
        self.assertEqual(job.exit_code, 0, job.stdout)
        self.assertEqual(self.executed(), MIXED_RUNNABLE, 'every runnable command, exactly once')
        verdicts = self.verdicts(job)
        self.assertEqual(sorted(verdicts), MIXED_RUNNABLE)
        self.assertEqual(verdicts['synthetic-coverage-percent']['actual'], 91)
        self.assertEqual(verdicts['synthetic-coverage-percent']['unit'], 'percent')
        self.assertEqual(verdicts['synthetic-bundle-bytes']['unit'], 'bytes')
        self.assertEqual(verdicts['synthetic-release-figure']['verdict'], 'over', 'report-only keeps its verdict')
        reasons = self.unavailable(job)
        self.assertEqual(sorted(reasons), ['synthetic-needs-a-tool', 'synthetic-needs-telemetry'])
        self.assertIn('needs tool-this-runner-lacks', reasons['synthetic-needs-a-tool'])
        self.assertIn('dist/telemetry/journey.json', reasons['synthetic-needs-telemetry'])
        self.assertNotIn('ci-report', ' '.join(reasons.values()), 'a label is never the reason')

    def test_an_enforced_budget_over_its_line_fails_and_a_second_head_updates_one_comment(self):
        first = self.pull_request()
        self.assertEqual(first.exit_code, 0, first.stdout)
        self.assertEqual(len(self.host.comments), 1)
        self.repository.write('packages/demo/bundle-size', '5000\n')
        second = self.pull_request_head()
        self.assertEqual(second.exit_code, 1, second.stdout)
        self.assertEqual(self.verdicts(second)['synthetic-bundle-bytes']['verdict'], 'over')
        self.assertEqual(self.executed().count('synthetic-bundle-bytes'), 1)
        self.assertEqual(len(self.host.comments), 1, 'the standing comment is updated, not repeated')
        self.assertIn(second.sha[:7], self.host.comments[0]['body'])

    def test_base_runs_are_compared_and_only_an_adverse_move_is_flagged(self):
        for _ in range(4):
            self.assertEqual(self.base_push().exit_code, 0)
        few = self.pull_request()
        self.assertEqual(few.rows()['synthetic-bundle-bytes']['baseline_n'], 4)
        self.assertIn('minimum 5', few.rows()['synthetic-bundle-bytes']['unavailable'])
        self.assertEqual(few.exit_code, 0, few.stdout)

        fifth = self.base_push()
        self.assertEqual(fifth.exit_code, 0)
        history = json.loads(self.host.artifact_files(fifth.run_id)['rolling-history.json'].decode())
        self.assertEqual(len(history['runs']), 5, 'one rolling file carries the base commits')
        self.repository.git('checkout', '--quiet', '-B', 'change', self.base_sha)
        self.repository.write('packages/demo/bundle-size', '900\n')
        worse = self.pull_request_head()
        self.assertEqual(worse.exit_code, 0, 'a flag fails nothing')
        row = worse.rows()['synthetic-bundle-bytes']
        self.assertEqual((row['verdict'], row['baseline_n'], row['flag']), ('within', 5, True))

        self.repository.git('checkout', '--quiet', '-B', 'change', self.base_sha)
        self.repository.write('packages/demo/bundle-size', '100\n')
        better = self.pull_request_head()
        row = better.rows()['synthetic-bundle-bytes']
        self.assertEqual((row['current'], row['flag']), (100, False), 'an improvement keeps its number, unflagged')
        self.assertEqual(len(self.host.comments), 1)

    def test_a_draft_reports_and_a_fork_gets_a_summary_and_artifact_without_credentials(self):
        draft = self.pull_request(draft=True)
        self.assertEqual(draft.exit_code, 0, draft.stdout)
        self.assertEqual(len(self.host.comments), 1)

        self.host.comments.clear()
        self.host.clear_requests()
        fork = self.pull_request(head_repository='someone/fork')
        self.assertEqual(fork.exit_code, 0, fork.stdout)
        self.assertEqual(self.host.comments, [])
        self.assertEqual(self.host.count('commits/main'), 0, 'a budget command reached the host')
        self.assertIn('someone/fork', fork.summary)
        self.assertIsNotNone(self.host.artifact_files(fork.run_id))

    def test_comment_and_history_failures_are_diagnostics_and_a_real_failure_survives(self):
        self.host.write_status = 403
        posted = self.pull_request()
        self.assertEqual(posted.exit_code, 0, posted.stdout)
        self.assertIn('not delivered', posted.summary)
        self.host.write_status = None

        self.host.refuse.append({'path': r'actions/runs/\d+$', 'status': 429, 'times': 1})
        unread = self.pull_request()
        self.assertEqual(unread.exit_code, 0, unread.stdout)
        self.assertIn('baseline unavailable', unread.markdown())

        self.host.write_status = 403
        self.repository.git('checkout', '--quiet', '-B', 'change', self.base_sha)
        self.repository.write('packages/demo/bundle-size', '5000\n')
        failed = self.pull_request_head()
        self.assertEqual(failed.exit_code, 1, failed.stdout)
        self.assertIn('not delivered', failed.summary)


class PredecessorOutcomes(Journey):
    """The committed job, started only if its real `needs` and `if:` start it for those outcomes."""

    OUTCOMES = {
        'every predecessor failed': dict.fromkeys(PREDECESSORS, 'failure'),
        'every predecessor skipped': dict.fromkeys(PREDECESSORS, 'skipped'),
        'one failed and one skipped': {'lint': 'success', 'test-linux': 'failure', 'test-windows': 'success',
                                       'test-macos': 'skipped'},
    }

    def setUp(self):
        super().setUp()
        self.write_real_catalog()
        self.write_mixed_catalog()
        self.base_sha = self.repository.commit('the real catalog and a runnable one')

    def run_scheduled_pull_request(self, results, bundle_size=500, workflow_file=ROOT / '.github/workflows/ci.yml'):
        self.repository.git('checkout', '--quiet', '-B', 'change', self.base_sha)
        self.repository.write('packages/demo/bundle-size', f'{bundle_size}\n')
        return self.start(workflow_file, 'pull_request', results, self.pull_request_head)

    def start(self, workflow_file, event_name, results, run):
        answer = schedule(workflow_file, event_name, results)
        self.assertTrue(answer['triggered'], f'{event_name} does not start the workflow')
        self.assertTrue(answer['selected'], f'budgets did not start with predecessors {results}: '
                                            f'its `needs` {answer["needs"]} and `if:` leave it skipped')
        # The job's steps are the checkout and the action, neither of which a failed predecessor may stop.
        self.assertEqual([step.get('if') for step in job_definition()['steps']], [None, None])
        return run()

    def assert_action_ran_and_saved(self, job):
        for step in ('report', 'main-upload', 'finish', 'final-upload'):
            self.assertEqual(job.outcomes[step], 'success', f'{step}: {job.stdout}')
        saved = json.loads(self.host.artifact_files(job.run_id)['budget-run.json'].decode())
        self.assertEqual(sorted(json.loads(self.host.artifact_files(job.run_id, 'budget-results-main')['budget-run.json'].decode())
                                ['results']['results'][0]), sorted(saved['results']['results'][0]))
        self.assertIn(OWN_BUDGET, {result['id'] for result in saved['results']['results']}, 'the completed verdict is saved')
        self.assertEqual(sorted(MIXED_RUNNABLE + [OWN_BUDGET]), sorted(result['id'] for result in saved['results']['results']))
        self.assertIn('stacked-spikes-stage-time', {entry['id'] for entry in saved['unavailable']})
        return {result['id']: result['verdict'] for result in saved['results']['results']}

    def test_the_action_runs_saves_its_results_and_keeps_enforcement_however_predecessors_end(self):
        for name, results in self.OUTCOMES.items():
            with self.subTest(name):
                passing = self.run_scheduled_pull_request(results)
                self.assertEqual(passing.exit_code, 0, passing.stdout)
                self.assertEqual(self.assert_action_ran_and_saved(passing)['synthetic-bundle-bytes'], 'within')
                self.assertEqual(sorted(self.executed()), MIXED_RUNNABLE, 'each ran once')

                over = self.run_scheduled_pull_request(results, bundle_size=5000)
                self.assertEqual(over.exit_code, 1, 'an enforced budget over its line still fails the job')
                self.assertEqual(self.assert_action_ran_and_saved(over)['synthetic-bundle-bytes'], 'over')
                self.assertEqual(len(self.host.comments), 1)

    def test_a_push_to_main_runs_it_with_every_predecessor_really_skipped(self):
        answer = schedule(ROOT / '.github/workflows/ci.yml', 'push', {})
        self.assertTrue(answer['selected'], answer)
        base = self.start(ROOT / '.github/workflows/ci.yml', 'push', {}, self.base_push)
        self.assertEqual(base.exit_code, 0, base.stdout)
        self.assert_action_ran_and_saved(base)

    def test_without_always_style_scheduling_the_job_would_not_start(self):
        """Held here so the journeys above cannot pass over a scheduler that starts it regardless."""
        committed = (ROOT / '.github/workflows/ci.yml').read_text()
        scheduling = '    needs: [lint, test-linux, test-windows, test-macos]\n    if: always()\n'
        self.assertEqual(committed.count(scheduling), 1)
        unscheduled = self.scratch / 'without-always.yml'
        unscheduled.write_text(committed.replace(scheduling, scheduling.split('    if:')[0]))
        for results in self.OUTCOMES.values():
            self.assertFalse(schedule(unscheduled, 'pull_request', results)['selected'], results)
        with self.assertRaisesRegex(AssertionError, 'budgets did not start with predecessors'):
            self.run_scheduled_pull_request(self.OUTCOMES['every predecessor failed'], workflow_file=unscheduled)
        self.assertTrue(schedule(unscheduled, 'pull_request', dict.fromkeys(PREDECESSORS, 'success'))['selected'])


class Scheduler(unittest.TestCase):
    def test_a_workflow_it_cannot_interpret_is_refused_and_one_without_the_job_is_named(self):
        script = str(HERE / 'ci-schedule.mjs')
        scenario = json.dumps({'eventName': 'push', 'ref': 'refs/heads/main', 'head': '0', 'event': PUSH_EVENT, 'results': {}})
        one_job = 'jobs: {lint: {runs-on: ubuntu-latest, steps: [{run: "true"}]}}\n'
        with tempfile.TemporaryDirectory() as directory:
            for name, content, code, said in (
                ('empty.yml', '', 2, 'not a workflow with jobs'),
                ('filtered.yml', f'name: x\non: {{push: {{paths: ["src/**"]}}}}\n{one_job}', 2, 'cannot interpret'),
                ('no-budgets.yml', f'name: x\non: {{push: {{branches: [main]}}}}\n{one_job}', 1, 'has no budgets job'),
            ):
                path = Path(directory) / name
                path.write_text(content)
                answer = subprocess.run(['node', script, str(path), scenario], cwd=ROOT, capture_output=True, text=True, check=False)
                self.assertEqual(answer.returncode, code, answer.stderr)
                self.assertIn(said, answer.stderr)

    def test_input_it_cannot_read_is_refused_as_invalid(self):
        script, ci = str(HERE / 'ci-schedule.mjs'), str(ROOT / '.github/workflows/ci.yml')
        for arguments in ([ci, 'not json'], ['/no/such/workflow.yml', '{}']):
            refused = subprocess.run(['node', script, *arguments], cwd=ROOT, capture_output=True, text=True, check=False)
            self.assertEqual(refused.returncode, 2, refused.stderr)
            self.assertIn('cannot read the workflow or the scenario', refused.stderr)

    def test_a_scenario_it_cannot_judge_is_refused(self):
        script, ci = str(HERE / 'ci-schedule.mjs'), str(ROOT / '.github/workflows/ci.yml')
        for scenario, named in (
            ({'eventName': 'push', 'ref': 'refs/heads/main', 'head': '0', 'event': PUSH_EVENT, 'results': {'lint': 'passed'}}, 'results'),
            ({'eventName': 'push', 'ref': 'refs/heads/main', 'head': '0', 'event': PUSH_EVENT, 'results': {'no-such-job': 'failure'}}, 'results'),
            ({'eventName': 'push', 'ref': 'refs/heads/main', 'head': '0', 'event': PUSH_EVENT, 'results': {'toString': 'failure'}}, 'results'),
            ({'eventName': 'pull_request', 'ref': 'r', 'head': '0', 'event': {}, 'results': {}}, 'event'),
            ({'eventName': 'schedule', 'ref': 'r', 'head': '0', 'event': PUSH_EVENT, 'results': {}}, 'eventName'),
        ):
            refused = subprocess.run(['node', script, ci, json.dumps(scenario)], cwd=ROOT, capture_output=True, text=True, check=False)
            self.assertEqual(refused.returncode, 2, refused.stderr)
            self.assertIn(f"scenario's {named}", refused.stderr)


class NamedInputs(unittest.TestCase):
    def test_each_input_a_command_names_is_the_one_it_reads(self):
        """A name no command reads would make the row's reason a lie, so garbage there must break it."""
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            (scratch / 'garbage.json').write_text('not json')
            for name in ('stop-guard-unpublished.json', 'stop-guard-unpublished-invocation.json',
                         'stop-guard-unpublished-load.json'):
                (scratch / name).write_text('not json')
            named = 0
            for budget, entry in catalog().items():
                cwd = (ROOT / entry['file']).parent
                command = list(entry['command'])
                if command[0] == 'just' or budget == OWN_BUDGET or entry.get('labels', [''])[0] in ('release-tail', 'host-variable'):
                    continue
                paths = [index for index, argument in enumerate(command) if index and '/' in argument and ' ' not in argument
                         and not argument.startswith('scripts/') and argument != '-c']
                self.assertEqual(len(paths), 1, f'{budget} names no one input: {command}')
                named += 1
                garbage = scratch / ('stop-guard-unpublished.json' if command[paths[0] - 1] == '--record' else 'garbage.json')
                command[paths[0]] = os.path.relpath(garbage, cwd)
                result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, check=False,
                                        env={**os.environ, 'ONEBUDGETSPEC_RESULT': str(scratch / 'result.json'),
                                             'ONEBUDGETSPEC_BUDGET_ID': budget})
                self.assertNotEqual(result.returncode, 0, f'{budget} did not read {garbage}: {result.stdout}')
                self.assertNotIn('ENOENT', result.stderr, budget)
                self.assertNotRegex(result.stderr, r'(?<!un)\bmissing\b', budget)
            self.assertGreater(named, 15)


class Pin(unittest.TestCase):
    def test_the_vendored_definition_is_the_release_the_workflow_pins(self):
        vendored = HERE / 'action-release' / TAG
        self.assertEqual((vendored / 'action.yml').read_bytes(), (ACTION_TREE / 'dist/action.yml').read_bytes())
        self.assertEqual((vendored / 'SHA256SUMS').read_bytes(), (ACTION_TREE / 'dist/SHA256SUMS').read_bytes())
        self.assertEqual((ACTION_TREE / 'action.yml').read_bytes(), (ACTION_TREE / 'dist/action.yml').read_bytes())
        metric = json.loads((vendored / 'report-metric.source.json').read_text())
        source = (ACTION_TREE / metric['source']).read_bytes()
        self.assertEqual(hashlib.sha256(source).hexdigest(), metric['source_sha256'])
        self.assertIn(f'"{metric["metric"]}"'.encode(), source)

    def test_the_vendored_reader_help_is_what_the_released_driver_prints(self):
        vendored = HERE / 'action-release' / TAG
        with tempfile.TemporaryDirectory() as directory, tarfile.open(next((ACTION_TREE / 'dist').glob('*.tar.gz'))) as archive:
            archive.extractall(directory, filter='data')
            driver = next(Path(directory).rglob('onebudgetspec-github-action'))
            shown = subprocess.run([str(driver), 'telemetry', '--help'], capture_output=True, check=True).stdout
        self.assertEqual(shown, (vendored / 'telemetry-help.txt').read_bytes())


class Workload(Journey):
    def test_record_a_complete_job(self):
        self.write_real_catalog()
        self.repository.write('synthetic-workload/budgets.yaml', fill_catalog(len(catalog())))
        heads = [self.repository.commit(f'base {index}') for index in range(COMMITS)]
        self.base_sha = heads[-1]
        first = self.run_job('push', heads[-1])
        self.assertEqual(first.exit_code, 0, first.stdout)

        # One real run, explicitly synthetic scalars copied once per real commit.
        history = {'schema_version': 1, 'repository': harness.REPOSITORY, 'base_ref': 'main', 'runs': [
            {**first.saved('budget-run.json'), 'head_sha': head, 'run_id': str(500 + index), 'attempt': 1}
            for index, head in enumerate(heads)]}
        self.host.artifacts.clear()
        self.host.runs.clear()
        self.host.add_run(519, heads[-1], minute=519)
        self.host.attach(519, 'budget-results', zip_of('rolling-history.json', json.dumps(history)))

        self.repository.git('checkout', '--quiet', '-b', 'feature')
        head = self.repository.commit('change')
        self.host.head = head
        figures = []
        for event_name, sha, options in (
            ('pull_request', head, {'base_ref': 'main', 'register': False, 'event': harness.event_pull_request(1, head, heads[-1])}),
            ('push', heads[-1], {'attempt': 2, 'register': False}),
        ):
            self.host.clear_requests()
            self.host.refuse.append({'path': r'artifacts\?', 'status': 429, 'times': 1, 'retry_after': 1})
            job = self.run_job(event_name, sha, **options)
            self.assertEqual(job.exit_code, 0, job.stdout)
            self.assertEqual(len(job.rows()), ROWS)
            self.assertEqual(job.outcomes['final-upload'], 'success', job.stdout)
            self.assertEqual(self.host.count('commits/'), COMMANDS_READING_REST, 'each runnable read once')
            telemetry = job.saved('report-job.json')
            requests = telemetry['metrics'][REQUESTS_METRIC]['value']
            self.assertIsNotNone(requests, telemetry)
            self.assertEqual(requests + self.host.requests_not_through_gh, self.host.count(''),
                             f'the job and the host disagree: {self.host.requests}')
            figures.append(self.boundary_record(telemetry))
            print(f'{event_name}: {self.host.count("")} GitHub API requests over the whole job, against the '
                  f'provisional design total of {DESIGN_REQUESTS}; {telemetry["metrics"][SECONDS_METRIC]["value"]} s',
                  file=sys.stderr)
        slowest = max(figures, key=lambda record: record['metrics'][SECONDS_METRIC]['value'])
        TELEMETRY.parent.mkdir(exist_ok=True)
        TELEMETRY.write_text(json.dumps(slowest))
        tools = TELEMETRY.parent / 'tools'
        tools.mkdir(exist_ok=True)
        shutil.copy2(job.temp / 'onebudgetspec-action/tools/onebudgetspec-github-action', tools)

    def boundary_record(self, telemetry):
        """The job's telemetry, with the complete traffic the boundary received beside it."""
        transfers = {service: self.host.transferred(service) for service in TRANSFER_SERVICES}
        record = json.loads(json.dumps(telemetry))
        record['boundary'] = {'github_api_requests': self.host.count(''),
                              'not_through_gh': self.host.requests_not_through_gh,
                              'file_transfers': transfers, 'design_github_api_requests': DESIGN_REQUESTS}
        return record


if __name__ == '__main__':
    unittest.main(verbosity=1)
