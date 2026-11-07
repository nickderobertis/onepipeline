"""This repository's `budgets` job, through the released action, and the telemetry it records.

Each journey reads the job out of `ci.yml` and runs what its `uses:` pins with its `with:` inputs:
action.yml's own steps, the released driver and plugin archive, the published core wheel, the real
`gh`, onevcs and `actions/upload-artifact` code. Only GitHub's API and artifact services, the release
host and the package index are doubled, by the harness the action ships at that same tag. The
checkout holds this repository's real budgets files and the commands they name, on a runner's PATH.
`test-linux`'s upload step runs from `ci.yml` too, through the same real upload code, over results the
pinned checker really saved.

The workload recorded for `onepipeline-budget-report-command-seconds`: the real catalog's 35 rows, one
rolling file of 20 commits and one saved-results artifact of about 50 KB, with a retried rate limit; a
base push also reads the merged pull request's run. It asserts behaviour and never a cost: the figures
are written to `.telemetry/report-job.json` and printed.
"""
import ast
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import re
import shlex
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
CHECKOUT_FILES = ('Cargo.toml', 'justfile', '.gitignore', '.github/.gitignore', '.github/macos-budget.mjs',
                  '.github/release-published-paths', '.github/telemetry.mjs',
                  '.github/workflow-graph.mjs', '.github/workflow-schema.mjs', *CATALOG_FILES)
CATALOG_ROWS = 35
COMMITS = 20
OMIT_LABEL = 'not-measured-on-pr'
OMITTED = ('onepipeline-cd-job-time', 'onepipeline-release-cycle-time', 'onepipeline-cd-job-count',
           'onepipeline-release-read-requests', 'onepipeline-pr-read-requests', 'onepipeline-nx-cache-use',
           'onepipeline-gate-time', 'onepipeline-pool-maintenance-seconds', 'driver-event-journal-delay',
           'driver-envelope-outcome-latency', 'driver-idle-cpu-share', 'driver-peak-rss',
           'liveness-journal-bytes-per-member-hour', 'pool-maintenance-engine-seconds',
           'run-ending-scratch-seconds', 'scratch-discard-seconds')
OWN_RUN_FIGURES = {'onepipeline-ci-check-time': 6549.0, 'onepipeline-ci-job-count': 5.0}
NO_SAVED_RESULT = 'no saved result was found on this run'
RESULTS_ARTIFACT = 'budget-check-results-test-linux'
# Hexadecimal digests zip to about half their size, so this many bytes of them make an artifact of
# about the 50 KB the budget's workload states.
RESULTS_ARTIFACT_BYTES = 100_000
OWN_RUN_JOBS = ROOT / '.github/tests/fixtures/own-run/jobs.json'
PUSH_EVENT = {'head_commit': {'message': 'feat: a change'}}
PREDECESSORS = ('lint', 'test-linux', 'test-windows', 'test-macos')
REQUESTS_METRIC = 'report-github-api-requests-per-run'
SECONDS_METRIC = 'report-command-seconds'
DESIGN_REQUESTS = 20
TELEMETRY = HERE / '.telemetry' / 'report-job.json'
TRANSFER_SERVICES = ('artifact-service', 'blob-storage', 'object-storage', 'release-host', 'package-index')
WRITE = 'echo "{\\"value\\": %s}" > "$ONEBUDGETSPEC_RESULT"'
# With no token for its host, gh tries the keyring, and that lookup autolaunches a session bus whose
# daemon outlives the job, holding one of the host's inotify instances.
NO_SESSION_BUS = {'DBUS_SESSION_BUS_ADDRESS': 'disabled:'}


def workflow():
    return yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())


def job_definition():
    return workflow()['jobs']['budgets']


def upload_step():
    """`test-linux`'s step that keeps what its `budgets` targets saved."""
    steps = [step for step in workflow()['jobs']['test-linux']['steps'] if step.get('with', {}).get('name') == RESULTS_ARTIFACT]
    assert len(steps) == 1, steps
    return steps[0]


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


class Host(harness.Host):
    """The action's GitHub double, answering one run's job listing with the recording of a real one."""

    def read(self, handler, path):
        if re.fullmatch(r'/repos/owner/repo/actions/runs/\d+/jobs', path.split('?')[0]):
            return self.reply(handler, 200, OWN_RUN_JOBS.read_bytes())
        return super().read(handler, path)


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


def recording_dbus_launch(directory, record):
    """A `dbus-launch` that notes each call and starts nothing, so a failing guard leaks nothing."""
    stand_in = directory / 'dbus-launch'
    stand_in.write_text(f'#!/bin/sh\necho called >> {shlex.quote(str(record))}\nexit 1\n')
    stand_in.chmod(0o755)


def zip_of(name, content):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w') as archive:
        archive.writestr(name, content)
    return buffer.getvalue()


class Journey(unittest.TestCase):
    def setUp(self):
        self.host = Host()
        self.addCleanup(self.host.close)
        self.repository = harness.Repository()
        self.addCleanup(self.repository.close)
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        (self.scratch / 'bin').mkdir()
        self.runner = runner_path(self.scratch / 'bin')
        self.bus_calls = self.scratch / 'bus-calls'
        recording_dbus_launch(self.scratch / 'bin', self.bus_calls)
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

    def job(self, event_name, sha, same_run=False, **options):
        self.next_run += 0 if same_run else 1
        self.counts.unlink(missing_ok=True)
        environment = {'JOURNEY_COUNTS': str(self.counts), 'PATH': self.runner, 'GITHUB_EVENT_NAME': event_name,
                       'GITHUB_REPOSITORY': harness.REPOSITORY, 'GITHUB_RUN_ID': str(self.next_run),
                       'RUNNER_NAME': 'hosted-runner', **NO_SESSION_BUS}
        job = harness.Job(self.host, self.repository, event_name, sha, inputs=self.inputs(), run_id=self.next_run,
                          environment=environment, action_path=ACTION_TREE, **options)
        self.addCleanup(job.close)
        return job

    def recorded_bus_calls(self):
        return len(self.bus_calls.read_text().splitlines()) if self.bus_calls.exists() else 0

    def run_job(self, event_name, sha, **options):
        result = self.job(event_name, sha, **options).run()
        self.assertEqual(self.recorded_bus_calls(), 0, 'gh asked for a session bus during the job')
        return result

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

    def upload_saved_results(self):
        """`test-linux`'s upload step from `ci.yml`, run as the runner takes it, through the real upload code."""
        step = upload_step()
        self.assertEqual(step['if'], 'always()', 'a failed job would lose what it saved')
        self.assertEqual(step['with']['path'], 'target/budget-results/')
        job = self.job('pull_request', '0' * 40)
        context = {'inputs': {}, 'github': job.github, 'steps': {}, 'runner': {'temp': str(job.temp)}}
        outcome, _ = job._artifact_action(step, harness.Expression(context), job.temp / 'upload-output')
        self.assertEqual(outcome, 'success', job.stdout)

    def pull_request_with_saved_results(self, **event):
        """A pull request's whole run: `test-linux` uploads, then `budgets` runs in the same run, in a fresh checkout."""
        self.repository.git('checkout', '--quiet', '-B', 'change', self.base_sha)
        head = self.repository.commit('change')
        self.host.head = head
        self.upload_saved_results()
        shutil.rmtree(self.repository.path / 'target', ignore_errors=True)
        (self.repository.path / 'node_modules').unlink(missing_ok=True)
        return self.run_job('pull_request', head, event=harness.event_pull_request(1, head, self.base_sha, **event),
                            base_ref='main', register=False, same_run=True)

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

    def shown(self):
        return sorted(set(catalog()) - set(OMITTED))

    def assert_no_row_left_out_of_the_report(self, job):
        self.assertEqual(job.exit_code, 0, job.stdout)
        self.assertEqual(sorted(job.rows()), self.shown())
        self.assertEqual([id for id, result in self.verdicts(job).items() if result['verdict'] == 'error'], [],
                         'a budget reads as an error because its input is absent')

    def test_the_label_covers_exactly_the_budgets_a_pull_request_cannot_measure(self):
        budgets = catalog()
        self.assertEqual(len(budgets), CATALOG_ROWS)
        self.assertEqual(sorted(id for id, budget in budgets.items() if OMIT_LABEL in budget.get('labels', [])), sorted(OMITTED))

    def test_a_pull_request_shows_its_own_run_and_says_why_every_other_row_has_no_figure(self):
        job = self.pull_request()
        self.assert_no_row_left_out_of_the_report(job)
        results = self.verdicts(job)
        own = results.pop(OWN_BUDGET)
        self.assertIn(own['verdict'], ('within', 'over'))
        self.assertEqual({id: result['actual'] for id, result in results.items()}, OWN_RUN_FIGURES)
        reasons = self.unavailable(job)
        self.assertEqual(sorted(reasons), sorted(set(self.shown()) - {OWN_BUDGET, *OWN_RUN_FIGURES}))
        self.assertEqual(set(reasons.values()), {NO_SAVED_RESULT})
        for id, reason in reasons.items():
            self.assertIsNone(job.rows()[id].get('current'), id)
        # Nothing was built, run or recorded to fill a row, and each of the pair read its one listing.
        self.assertFalse((self.repository.path / 'target').exists())
        self.assertEqual(self.host.count('/releases|/compare'), 0)
        self.assertEqual(self.host.count(r'/runs/\d+/jobs'), len(OWN_RUN_FIGURES))
        self.assertEqual(len(self.host.comments), 1)
        for budget in self.shown():
            self.assertIn(budget, self.host.comments[0]['body'])
        self.assertEqual(self.budget_run(job)['enforcement_exclude_labels'], ['release-tail', 'host-variable'])
        # The job's own budget ran after the main upload, and its completed verdict was saved.
        self.assertEqual(job.outcomes['main-upload'], 'success', job.stdout)
        self.assertEqual(job.outcomes['final-upload'], 'success', job.stdout)
        saved = json.loads(self.host.artifact_files(job.run_id)['budget-run.json'].decode())
        own = {result['id']: result for result in saved['results']['results']}[OWN_BUDGET]
        self.assertEqual(own['unit'], 'seconds')
        self.assertIsNotNone(self.host.artifact_files(job.run_id, 'budget-results-main'))

    def test_the_labelled_budgets_have_no_row_run_no_command_and_one_line_counts_them(self):
        document = yaml.safe_load((self.repository.path / 'budgets.yaml').read_text())
        for budget in document['budgets']:
            budget['command'] = counting(WRITE % 1)
        self.repository.write('budgets.yaml', yaml.safe_dump(document))
        for name in ('pool-maintenance', 'tests/journal_workload', 'tests/pool_workload', 'tests/scratch_workload'):
            document = yaml.safe_load((self.repository.path / name / 'budgets.yaml').read_text())
            for budget in document['budgets']:
                budget['command'] = counting(WRITE % 1)
            self.repository.write(f'{name}/budgets.yaml', yaml.safe_dump(document))
        self.base_sha = self.repository.commit('every command counts itself')
        job = self.pull_request()
        ran = set(self.executed())
        self.assertEqual(ran & set(OMITTED), set(), 'a labelled budget ran its command')
        self.assertTrue(ran, 'the counting commands of the budgets it shows never ran')
        self.assertEqual(set(job.rows()) & set(OMITTED), set())
        body = self.host.comments[0]['body']
        line = f'{len(OMITTED)} budgets are left out because a pull request cannot measure them (label {OMIT_LABEL}).'
        self.assertEqual(body.count(line), 1, body)
        for budget in OMITTED:
            self.assertNotIn(budget, body)
        self.assertEqual(sorted(entry['id'] for entry in self.budget_run(job)['catalog'] if OMIT_LABEL in entry.get('labels', [])),
                         sorted(OMITTED), "the catalog keeps them")

    def test_a_saved_result_for_a_labelled_budget_is_ignored(self):
        results = self.repository.path / 'target/budget-results'
        results.mkdir(parents=True)
        (results / 'stray.json').write_text(harness.check_report(
            harness.check_result(OMITTED[0], 5, labels=['release-tail', OMIT_LABEL], unit='seconds')))
        job = self.pull_request_with_saved_results()
        self.assertNotIn(OMITTED[0], job.rows())

    def test_a_base_push_with_no_pull_request_saves_the_same_results_and_posts_nothing(self):
        pull = self.pull_request()
        posted = len(self.host.comments)
        base = self.base_push()
        self.assertEqual(base.exit_code, 0, base.stdout)
        self.assertEqual(sorted(base.rows()), self.shown())
        self.assertEqual(len(self.host.comments), posted)
        base_files = self.host.artifact_files(base.run_id)
        pull_files = self.host.artifact_files(pull.run_id)
        self.assertEqual(set(base_files), {*pull_files, 'rolling-history.json'}, 'only history is added')
        saved = json.loads(base_files['budget-run.json'].decode())
        self.assertEqual([result['id'] for result in saved['results']['results']], [OWN_BUDGET])
        self.assertEqual(self.host.count(r'/runs/\d+/jobs'), len(OWN_RUN_FIGURES), "only the pull request's run was read")

    def test_a_base_push_saves_the_own_budget_as_history_a_later_pull_request_reads_as_a_baseline(self):
        base = self.base_push()
        self.assertEqual(base.exit_code, 0, base.stdout)
        files = self.host.artifact_files(base.run_id)
        history = json.loads(files['rolling-history.json'].decode())
        self.assertEqual([run['head_sha'] for run in history['runs']], [self.base_sha])
        kept = {result['id']: result for result in history['runs'][0]['results']['results']}
        self.assertEqual(list(kept), [OWN_BUDGET], 'the only measurement this catalog has is what the history holds')
        self.assertIsNotNone(kept[OWN_BUDGET]['actual'], 'the entry was saved before the budget was measured')

        pull = self.pull_request()
        self.assertEqual(pull.exit_code, 0, pull.stdout)
        row = pull.rows()[OWN_BUDGET]
        self.assertEqual(row['baseline_n'], 1, row)
        self.assertIn('minimum 5', row['unavailable'], 'one sample is read, and is too few to compare yet')

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


class SavedResults(Journey):
    """`test-linux` saves through the pinned checker, its upload step ships that, and `budgets` shows it."""

    def setUp(self):
        super().setUp()
        self.write_real_catalog()
        self.base_sha = self.repository.commit('the real catalog')
        (self.repository.path / 'node_modules').symlink_to(ROOT / 'node_modules')
        tools = self.repository.path / 'target'
        tools.mkdir(exist_ok=True)
        (tools / 'tools').symlink_to(ROOT / 'target/tools')
        records = tools / 'budget-records'
        records.mkdir()
        (records / 'stacked-spikes-stage.json').write_text('{"wall_ms": 1234, "run_id": "run"}')
        (records / 'human-items.json').write_text(json.dumps({
            'host': 'runner', 'warm_max_elapsed_s': 0.25, 'warm': [{'elapsed_s': 0.25, 'phase': 'warm', **dict.fromkeys(
                ('index_io_ms', 'discovery_ms', 'replay_ms', 'blocker_ms', 'render_ms', 'capture_ms'), 1)}],
            'cold': [], 'tail': [], 'workload': dict.fromkeys(('runs', 'events_per_run', 'open_items', 'plans', 'steps'), 1)}))

    def checker(self, *arguments, cwd=None):
        """The pinned checker with the flag a saving target passes, leaving exactly what that target leaves."""
        root = subprocess.run(['just', '--evaluate', 'onebudgetspec-root'], cwd=ROOT, capture_output=True, text=True, check=True).stdout
        return subprocess.run([f'{root}/bin/onebudgetspec', 'check', *arguments], cwd=cwd or self.repository.path, capture_output=True,
                              text=True, check=False)

    def save(self, *arguments, name='onepipeline.json', cwd=None, status=0):
        ran = self.checker(*arguments, '--save-results', str(self.repository.path / 'target/budget-results' / name), cwd=cwd)
        self.assertEqual(ran.returncode, status, ran.stdout + ran.stderr)
        return json.loads((self.repository.path / 'target/budget-results' / name).read_text())

    def saved_figures(self):
        stacked = self.save('budgets.yaml', '--id', 'stacked-spikes-stage-time')
        human = self.save('budgets.yaml', name='human-items.json', cwd=self.repository.path / 'tests/human_items')
        return {result['id']: result for result in [*stacked['results'], *human['results']]}

    def test_a_saved_figure_shows_exactly_and_a_budget_with_none_says_so(self):
        saved = self.saved_figures()
        self.assertEqual(sorted(saved), ['human-items-read-seconds', 'stacked-spikes-stage-time'])
        job = self.pull_request_with_saved_results()
        self.assertEqual(job.exit_code, 0, job.stdout)
        self.assertEqual(job.outcomes['saved-results'], 'success', job.stdout)
        rows = job.rows()
        for id, result in saved.items():
            self.assertEqual(rows[id]['current'], result['actual'], id)
            self.assertEqual(rows[id]['verdict'], result['verdict'], id)
        self.assertEqual(rows['stacked-spikes-stage-time']['current'], 1.234)
        self.assertEqual(rows['human-items-read-seconds']['current'], 0.25)
        for id, figure in OWN_RUN_FIGURES.items():
            self.assertEqual(rows[id]['current'], figure, 'the CI-time pair comes from the job')
        body = self.host.comments[0]['body']
        for absent in ('host-view-time', 'onepipeline-macos-jobs-per-run', 'review-round-github-quota'):
            self.assertIsNone(rows[absent].get('current'), absent)
            self.assertEqual(self.unavailable(job)[absent], NO_SAVED_RESULT, absent)
            line = next(line for line in body.splitlines() if absent in line)
            self.assertIn(NO_SAVED_RESULT, line)

    def test_a_saved_over_budget_keeps_its_verdict_and_fails_the_job(self):
        (self.repository.path / 'target/budget-records/stacked-spikes-stage.json').write_text('{"wall_ms": 600000, "run_id": "run"}')
        self.save('budgets.yaml', '--id', 'stacked-spikes-stage-time', status=1)
        job = self.pull_request_with_saved_results()
        self.assertEqual(job.exit_code, 1, 'an enforced budget saved over its line still fails this job')
        self.assertEqual(job.rows()['stacked-spikes-stage-time']['verdict'], 'over')
        self.assertEqual(job.rows()['stacked-spikes-stage-time']['current'], 600.0)

    def test_a_failed_test_job_that_saved_nothing_reads_as_no_saved_result(self):
        # `if-no-files-found: warn`: the step passes with nothing to ship, and no artifact arrives.
        shutil.rmtree(self.repository.path / 'target/budget-results', ignore_errors=True)
        job = self.pull_request_with_saved_results()
        self.assertEqual(job.exit_code, 0, job.stdout)
        self.assertNotIn(RESULTS_ARTIFACT, [item['name'] for item in self.host.artifacts[str(job.run_id)]])
        self.assertEqual(self.unavailable(job)['stacked-spikes-stage-time'], NO_SAVED_RESULT)

    def test_a_base_push_records_the_merged_pull_requests_figures_with_no_test_job_and_no_command(self):
        answer = schedule(ROOT / '.github/workflows/ci.yml', 'push', {})
        self.assertEqual(answer['jobs'], {**dict.fromkeys(PREDECESSORS, False), 'budgets': True})
        saved = self.saved_figures()
        pull = self.pull_request_with_saved_results()
        head = pull.sha
        self.assertEqual(pull.exit_code, 0, pull.stdout)
        self.host.add_run(pull.run_id, head, event='pull_request', minute=1)
        merged = {result['id']: result for result in pull.saved('budget-run.json')['results']['results']}

        self.repository.git('checkout', '--quiet', 'main')
        # Every command of the merged run's budgets now reports what would give it away if it ran.
        for name, entries in {'budgets.yaml': ['stacked-spikes-stage-time', 'onepipeline-ci-check-time', 'onepipeline-ci-job-count'],
                              'tests/human_items/budgets.yaml': ['human-items-read-seconds']}.items():
            document = yaml.safe_load((self.repository.path / name).read_text())
            for budget in document['budgets']:
                if budget['id'] in entries:
                    budget['command'] = counting(WRITE % 999)
            self.repository.write(name, yaml.safe_dump(document))
        merge = self.repository.commit('squash')
        self.host.merge_pull_request(merge, head)
        self.host.clear_requests()
        base = self.run_job('push', merge)
        self.assertEqual(base.exit_code, 0, base.stdout)
        self.assertEqual(self.executed(), [], "a budget's command ran on the base push")
        self.assertEqual(self.host.count(f'/commits/{merge}/pulls$'), 1)
        history = json.loads(self.host.artifact_files(base.run_id)['rolling-history.json'].decode())
        entry = next(run for run in history['runs'] if run['head_sha'] == merge)
        kept = {result['id']: result for result in entry['results']['results']}
        for id, result in saved.items():
            self.assertEqual(kept[id]['actual'], result['actual'], id)
            self.assertEqual(kept[id]['verdict'], result['verdict'], id)
        for id, figure in OWN_RUN_FIGURES.items():
            self.assertEqual(kept[id]['actual'], figure, id)
            self.assertEqual(kept[id]['actual'], merged[id]['actual'], id)
        self.assertFalse(set(kept) & set(OMITTED))
        self.assertNotEqual(kept[OWN_BUDGET]['actual'], merged[OWN_BUDGET]['actual'], 'the own budget stays this job')


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
        self.assertEqual(set(reasons.values()), {NO_SAVED_RESULT})
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
        figures = {*OWN_RUN_FIGURES} if job.event_name == 'pull_request' else set()
        self.assertEqual(sorted({*MIXED_RUNNABLE, OWN_BUDGET, *figures}), sorted(result['id'] for result in saved['results']['results']))
        self.assertIn('stacked-spikes-stage-time', {entry['id'] for entry in saved['unavailable']})
        self.assertFalse(set(OMITTED) & {entry['id'] for entry in saved['unavailable']})
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


class SessionBus(Journey):
    """The real `gh` under what the job is handed, at the double, against a recording `dbus-launch`."""

    def gh_under(self, job, environment):
        """How many times one real `gh` call asks for a bus, with what `_shell` adds to a job's environment."""
        self.bus_calls.unlink(missing_ok=True)
        subprocess.run(['gh', 'api', 'repos/owner/repo/commits/main'], capture_output=True, check=False, timeout=60,
                       env={'HOME': str(job.temp), **harness.real_gh_environment(self.host.url, job.temp), **environment})
        return self.recorded_bus_calls()

    def test_the_job_environment_keeps_the_real_gh_from_asking_for_a_session_bus(self):
        if platform.system() != 'Linux' or shutil.which('gh') is None:
            self.skipTest('no session bus can start here: this guard needs the real gh on Linux')
        job = self.job('push', '0' * 40)
        without = {key: value for key, value in job.environment.items() if key != 'DBUS_SESSION_BUS_ADDRESS'}
        if self.gh_under(job, without) == 0:
            self.skipTest('this gh asks for no session bus even without the variable, so none can leak')
        self.assertEqual(self.gh_under(job, job.environment), 0, 'the job environment lets gh ask for a session bus')

    def test_every_job_is_built_where_that_environment_and_the_recording_are(self):
        tree = ast.parse(Path(__file__).read_text())
        jobs = [node for node in ast.walk(tree) if isinstance(node, ast.Call) and ast.unparse(node.func) == 'harness.Job']
        self.assertEqual(len(jobs), 1, 'a second harness.Job would hand the real gh an environment nothing here guards')


class Workload(Journey):
    def test_record_a_complete_job(self):
        self.write_real_catalog()
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

        saved = sorted(set(catalog()) - set(OMITTED) - {OWN_BUDGET, *OWN_RUN_FIGURES})
        details = [hashlib.sha256(str(index).encode()).hexdigest() for index in range(RESULTS_ARTIFACT_BYTES // 64)]
        each = len(details) // len(saved)
        results = self.repository.path / 'target/budget-results'
        results.mkdir(parents=True)
        (results / 'test-linux.json').write_text(harness.check_report(*[
            harness.check_result(budget, catalog()[budget]['threshold'], file=catalog()[budget]['file'], labels=catalog()[budget].get('labels', ()),
                                 unit=catalog()[budget]['unit'], threshold=catalog()[budget]['threshold'],
                                 detail=''.join(details[index * each:(index + 1) * each]))
            for index, budget in enumerate(saved)]))

        self.repository.git('checkout', '--quiet', '-b', 'feature')
        head = self.repository.commit('change')
        self.host.head = head
        figures = []
        for event_name, sha, options in (
            ('pull_request', head, {'base_ref': 'main', 'register': False, 'event': harness.event_pull_request(1, head, heads[-1])}),
            ('push', None, {'attempt': 2, 'register': False}),
        ):
            self.host.clear_requests()
            self.host.refuse.append({'path': r'artifacts\?', 'status': 429, 'times': 1, 'retry_after': 1})
            if event_name == 'pull_request':
                self.upload_saved_results()
                size = next(item['size_in_bytes'] for item in self.host.artifacts[str(self.next_run)]
                            if item['name'] == RESULTS_ARTIFACT)
                print(f'{RESULTS_ARTIFACT}: {size} zipped bytes', file=sys.stderr)
                options['same_run'] = True
                shutil.rmtree(results)
            else:
                self.host.add_run(self.next_run, head, event='pull_request', minute=600)
                self.repository.git('checkout', '--quiet', 'main')
                sha = self.repository.commit('squash')
                self.host.merge_pull_request(sha, head)
            job = self.run_job(event_name, sha, **options)
            self.assertEqual(job.exit_code, 0, job.stdout)
            self.assertEqual(len(self.budget_run(job)['catalog']), CATALOG_ROWS)
            self.assertEqual(len(job.rows()), CATALOG_ROWS - len(OMITTED))
            self.assertEqual(job.outcomes['final-upload'], 'success', job.stdout)
            for budget in saved:
                self.assertEqual(job.rows()[budget]['current'], catalog()[budget]['threshold'], budget)
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
