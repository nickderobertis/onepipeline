"""Fold samply CPU stacks; sampled CPU is attribution, never a hosted wall-time estimate."""
import argparse
import bisect
import collections
import gzip
import json
import math
import re
import sys
from pathlib import Path


def read_json(path):
    opener = gzip.open if str(path).endswith('.gz') else open
    with opener(path, 'rt', encoding='utf-8') as stream:
        return json.load(stream, parse_constant=lambda value: invalid(f'non-finite JSON number {value}'))


def invalid(message):
    raise ValueError(message)


def at(values, index, label):
    if not isinstance(values, list) or type(index) is not int or not 0 <= index < len(values):
        invalid(f'{label}: invalid table index')
    return values[index]


def number(value, label):
    try:
        valid = type(value) in (int, float) and math.isfinite(value) and value >= 0
    except OverflowError:
        valid = False
    if not valid:
        invalid(f'{label}: expected a finite nonnegative number')
    return value


def mapping(value, label):
    if not isinstance(value, dict):
        invalid(f'{label}: expected an object')
    return value


def sequence(value, label):
    if not isinstance(value, list):
        invalid(f'{label}: expected an array')
    return value


def text(value, label):
    if not isinstance(value, str):
        invalid(f'{label}: expected text')
    return value


def fold(profile, symbols):
    profile = mapping(profile, 'profile')
    symbols = mapping(symbols, 'symbols')
    mapping(profile['meta'], 'profile metadata')
    if profile['meta']['preprocessedProfileVersion'] != 49:
        version = (Path(__file__).resolve().parent.parent / 'spikes/engine-startup/samply-version.txt').read_text().strip()
        invalid(f'expected samply {version} processed profile format 49; record with the pinned profiler')
    if profile['meta']['sampleUnits']['threadCPUDelta'] != 'µs':
        raise ValueError('expected samply threadCPUDelta in microseconds')
    strings = sequence(symbols['string_table'], 'symbol strings')
    tables = {}
    for entry in sequence(symbols['data'], 'symbol data'):
        entry = mapping(entry, 'symbol entry')
        tables[(text(entry['debug_name'], 'symbol debug name'), text(entry['debug_id'], 'symbol debug id').replace('-', '').upper())] = sorted(
            (number(s['rva'], 'symbol rva'), number(s['size'], 'symbol size'), at(strings, s['symbol'], 'symbol string')) for s in (mapping(row, 'symbol row') for row in sequence(entry['symbol_table'], 'symbol table')))

    def symbol(lib, address):
        table = tables.get((lib['debugName'], lib['breakpadId'][:32]), [])
        at = bisect.bisect_right(table, (address, float('inf'), '')) - 1
        if at >= 0 and table[at][0] <= address < table[at][0] + max(table[at][1], 1):
            return re.sub(r'::h[0-9a-f]{16}$', '', table[at][2])
        return lib['debugName'] + '!unresolved'

    processes = collections.defaultdict(list)
    times = {}
    for thread in sequence(profile['threads'], 'threads'):
        thread = mapping(thread, 'thread')
        if thread.get('processName') == 'onepipeline':
            pid = thread['pid']
            if not isinstance(pid, str) or re.fullmatch(r'[1-9][0-9]*(?:\.[0-9]+)?', pid) is None:
                invalid('process id: expected the pinned profiler’s positive PID string')
            start, end = thread.get('processStartupTime'), thread.get('processShutdownTime')
            if start is not None:
                number(start, 'process start')
            if end is not None:
                number(end, 'process end')
            known = times.setdefault(pid, [None, None])
            for index, value in enumerate((start, end)):
                if value is not None:
                    if known[index] is not None and value != known[index]:
                        invalid('contradictory process timestamps across threads')
                    known[index] = value
            if known[0] is not None and known[1] is not None and known[1] < known[0]:
                invalid('process end precedes its start')
            processes[pid].append(thread)
    output = []
    for pid, threads in processes.items():
        total = 0
        functions = collections.Counter()
        modules = collections.Counter()
        leaves = collections.Counter()
        for thread in threads:
            frames = thread['frameTable']
            stacks = thread['stackTable']
            names = {}

            def name(frame):
                if frame not in names:
                    resource = at(thread['funcTable']['resource'], at(frames['func'], frame, 'frame function'), 'function resource')
                    lib = at(thread['resourceTable']['lib'], resource, 'resource library') if resource is not None and resource != -1 else None
                    names[frame] = symbol(at(profile['libs'], lib, 'library'), number(at(frames['address'], frame, 'frame address'), 'frame address')) if lib is not None and at(frames['address'], frame, 'frame address') not in (None, -1) else 'unresolved'
                return names[frame]

            samples = thread['samples']
            length = samples['length']
            if type(length) is not int or length < 0 or any(len(samples[key]) != length for key in ('threadCPUDelta', 'stack')):
                invalid('sample columns do not match their declared length')
            for i in range(length):
                cpu = number(samples['threadCPUDelta'][i] if samples['threadCPUDelta'][i] is not None else 0, 'sample CPU')
                if not cpu:
                    continue
                total += cpu
                stack = samples['stack'][i]
                chain = []
                visited = set()
                while stack is not None:
                    if type(stack) is not int or stack in visited:
                        invalid('stack has an invalid index or cycle')
                    visited.add(stack)
                    chain.append(name(at(stacks['frame'], stack, 'stack frame')))
                    stack = at(stacks['prefix'], stack, 'stack prefix')
                leaf = chain[0] if chain else 'unresolved (no stack)'
                leaves[leaf] += cpu
                owner = next((n for n in chain if re.match(r'^<?(?:one[a-z_]*|privateterms)::', n)), leaf)
                functions[owner] += cpu
                modules['::'.join(owner.lstrip('<').split('::')[:2])] += cpu
        start, end = times[pid]
        life = f'{end-start:.3f} ms' if start is not None and end is not None else 'not measured'
        # llmlint: ignore-block[tool_output_is_signal] The requested never-land CPU attribution report needs rows reaching 80 percent, with unresolved frames and uncertainty visible; a single success line loses that evidence (manager a1).
        output.append(f'pid {pid}: sampled CPU {total/1000:.3f} ms; lifetime {life}; threads {len(threads)}')
        for title, counts in [('nearest first-party module (otherwise leaf)', modules), ('nearest first-party function (otherwise leaf)', functions), ('self function', leaves)]:
            output.append(title + ':')
            accumulated = 0
            for key, cpu in counts.most_common():
                accumulated += cpu
                output.append(f'  {cpu/1000:.3f} ms ({cpu/total*100:.2f}%; cumulative {accumulated/total*100:.2f}%) {key}')
                if accumulated >= total * .8:
                    break
        output.append('CPU attribution only: child lifetimes, filesystem latency, waits and process creation are not disjointly measured here.')
        # llmlint: ignore-end[tool_output_is_signal]
    if not output:
        raise ValueError('no onepipeline process found; record the real e2e journey with subprocess sampling')
    return '\n'.join(output) + '\n'


if __name__ == '__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    sys.stderr.reconfigure(encoding='utf-8')
    parser = argparse.ArgumentParser(description=__doc__ + ' Exit codes: 0 report produced; 2 malformed input or command usage, with a repair on stderr.')
    parser.add_argument('profile')
    parser.add_argument('symbols')
    args = parser.parse_args()
    try:
        print(fold(read_json(args.profile), read_json(args.symbols)), end='')
    except (ValueError, KeyError, OSError, TypeError, IndexError, AttributeError, EOFError) as error:
        parser.exit(2, f'engine-startup-cpu: {error}; supply a samply profile and its presymbolicated sidecar\n')
