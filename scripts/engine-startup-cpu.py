"""Fold samply CPU stacks; sampled CPU is attribution, never a hosted wall-time estimate."""
import argparse
import bisect
import collections
import gzip
import json
import re


def read_json(path):
    opener = gzip.open if str(path).endswith('.gz') else open
    with opener(path, 'rt') as stream:
        return json.load(stream)


def fold(profile, symbols):
    if profile['meta']['sampleUnits']['threadCPUDelta'] != 'µs':
        raise ValueError('expected samply threadCPUDelta in microseconds')
    strings = symbols['string_table']
    tables = {}
    for entry in symbols['data']:
        tables[(entry['debug_name'], entry['debug_id'].replace('-', '').upper())] = sorted(
            (s['rva'], s['size'], strings[s['symbol']]) for s in entry['symbol_table'])

    def symbol(lib, address):
        table = tables.get((lib['debugName'], lib['breakpadId'][:32]), [])
        at = bisect.bisect_right(table, (address, float('inf'), '')) - 1
        if at >= 0 and table[at][0] <= address < table[at][0] + max(table[at][1], 1):
            return re.sub(r'::h[0-9a-f]{16}$', '', table[at][2])
        return lib['debugName'] + '!unresolved'

    processes = collections.defaultdict(list)
    for thread in profile['threads']:
        if thread.get('processName') == 'onepipeline':
            processes[thread['pid']].append(thread)
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
                    resource = thread['funcTable']['resource'][frames['func'][frame]]
                    lib = thread['resourceTable']['lib'][resource] if resource is not None and resource >= 0 else None
                    names[frame] = symbol(profile['libs'][lib], frames['address'][frame]) if lib is not None and frames['address'][frame] not in (None, -1) else 'unresolved'
                return names[frame]

            samples = thread['samples']
            for i in range(samples['length']):
                cpu = samples['threadCPUDelta'][i] or 0
                if not cpu:
                    continue
                total += cpu
                stack = samples['stack'][i]
                chain = []
                while stack is not None:
                    chain.append(name(stacks['frame'][stack]))
                    stack = stacks['prefix'][stack]
                leaf = chain[0] if chain else 'unresolved (no stack)'
                leaves[leaf] += cpu
                owner = next((n for n in chain if re.match(r'^<?(?:one[a-z_]*|privateterms)::', n)), leaf)
                functions[owner] += cpu
                modules['::'.join(owner.lstrip('<').split('::')[:2])] += cpu
        first = threads[0]
        start, end = first.get('processStartupTime'), first.get('processShutdownTime')
        life = f'{end-start:.3f} ms' if start is not None and end is not None else 'not measured'
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
    if not output:
        raise ValueError('no onepipeline process found; record the real e2e journey with subprocess sampling')
    return '\n'.join(output) + '\n'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('profile')
    parser.add_argument('symbols')
    args = parser.parse_args()
    try:
        print(fold(read_json(args.profile), read_json(args.symbols)), end='')
    except (ValueError, KeyError, OSError, TypeError) as error:
        parser.exit(2, f'engine-startup-cpu: {error}; supply a samply profile and its presymbolicated sidecar\n')
