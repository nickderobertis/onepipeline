#!/usr/bin/env python3
"""The figures the report quotes that analyse.py does not print: warm runs inside the watch, what eviction
took and how old it was, end-of-watch room under 10 GiB, per-run crossing shares, and last-40 base moves.

    python3 scripts/spike-cache-eviction/report_figures.py DIR   # DIR holds readings.jsonl and hist/
"""
import collections, os, pathlib, sys
from datetime import datetime, timezone
sys.path.insert(0, str(pathlib.Path(__file__).parent))
import analyse as a, history as h

d = pathlib.Path(sys.argv[1])
s = h.Store(d / "hist")
# A run still in progress when its jobs were first cached left them unfinished there.
s.forget(f"repos/{h.OWNER}/onepipeline/actions/runs/42258425963/jobs?per_page=100&filter=latest")
since=datetime(2026,11,6,17,0,tzinfo=timezone.utc)
for repo,w in (('onepipeline','rust-cache-warm.yml'),('onetaskgraph','rust-cache-warm.yml'),('onepipeline-ui','ci-cache.yml')):
    runs=h.workflow_runs(s,repo,w,since=since,fresh=True)
    print(repo,'warm runs in window',len(runs))
    for run in runs:
        print(' run',run['id'],run['created_at'][5:19],run['updated_at'][11:19],run['conclusion'],run['head_commit']['message'].splitlines()[0][:40])
        for j in h.jobs(s,repo,run['id']):
            if j.get('conclusion') in (None,'skipped'): continue
            sv=a.saves(s.log(repo,j['id']) or '')
            print('   ',j['name'][:22],j['started_at'][11:19],j['completed_at'][11:19],[(x['finished'][11:19],round(x['bytes']/1e6)) for x in sv if x['bytes']>5e6], a.rust_restore(s.log(repo,j['id']) or '')[:60])
# Results' age when evicted
prev=None
rows=a.readings(d/'readings.jsonl','onetaskgraph')
ages=collections.defaultdict(list); access=collections.defaultdict(list)
for x,y in zip(rows,rows[1:]):
    A={e['id']:e for e in x['entries']};B={e['id'] for e in y['entries']}
    for i,e in A.items():
        if i not in B:
            k=(a.kind(e),a.scope(e))
            ages[k].append(a.minutes(y['started_at'],e['created_at']))
            access[k].append(a.minutes(y['started_at'],e['last_accessed_at']))
for k in ages: print(k,'evicted',len(ages[k]),'age created min',a.summary(ages[k]),'idle min',a.summary(access[k]))
# residual: what the families are now
for repo in a.REPOS:
    r=a.readings(d/'readings.jsonl',repo)[-1]
    fam=collections.Counter();cnt=collections.Counter()
    for e in r['entries']:
        k=(a.scope(e),a.family(e['key']) if e['key'].startswith(('v0-rust','ci-cap','boot')) else a.kind(e))
        fam[k]+=e['size_in_bytes'];cnt[k]+=1
    print(repo,r['started_at'],round(r['listed_bytes']/1e9,3),'headroom to 10GiB',round((a.GIB-r['listed_bytes'])/1e9,3))
    for k,v in fam.most_common(14): print('  ',k,cnt[k],round(v/1e9,3))
# per-run Rust saved bytes thresholds
for repo in a.REPOS:
    sv=[e for e in a.warm_saves(s,repo) if e['bytes']>50e6]
    by=collections.defaultdict(int)
    for e in sv: by[e['run']]+=e['bytes']
    vals=[v/1e9 for v in by.values()]
    print(repo,'runs with save',len(vals),'p50',round(a.pct(vals,.5),2),'p90',round(a.pct(vals,.9),2),'max',round(max(vals),2))
    latest={}
    for e in sv: latest[a.family(e['key'])]=e
    print('   latest per family',{k:round(v['bytes']/1e9,3) for k,v in latest.items()},'sum',round(sum(v['bytes'] for v in latest.values())/1e9,3))

rows=a.readings(d/'readings.jsonl','onetaskgraph')
# all saves in the window with their running total, and evict brackets
seen={}
for r in rows:
    for e in r['entries']:
        seen.setdefault(e['id'],e)
for lo,hi in (('18:20','18:43'),('19:12','19:35'),('00:08','00:11'),('00:24','00:36')):
    day='2026-11-07' if lo.startswith('00') else '2026-11-06'
    es=sorted([e for e in seen.values() if e['created_at'][:10]==day and lo<=e['created_at'][11:16]<=hi],key=lambda e:e['created_at'])
    print(day,lo,hi,[(e['created_at'][11:19],round(e['size_in_bytes']/1e6,1)) for e in es][:40])
# inter-save gap per repo from the 8h listing and from main hours
for repo in a.REPOS:
    rs=a.readings(d/'readings.jsonl',repo)
    all_e={}
    for r in rs:
        for e in r['entries']: all_e.setdefault(e['id'],e)
    created=sorted(e['created_at'] for e in all_e.values() if e['created_at']>='2026-11-06T17:06')
    gaps=[a.minutes(b,x) for x,b in zip(created,created[1:])]
    print(repo,'saves in watch',len(created),'gap min',a.summary(gaps) if gaps else None, 'share of gaps under 1 min', round(sum(g<1 for g in gaps)/max(1,len(gaps)),2))
    last=rs[-1]
    fam=collections.Counter()
    for e in last['entries']:
        fam[a.scope(e)]+=e['size_in_bytes']
    print('  by scope GB',{k:round(v/1e9,3) for k,v in fam.items()},'results GB',round(sum(e['size_in_bytes'] for e in last['entries'] if a.kind(e)=='nx-cache')/1e9,4),'n',sum(a.kind(e)=='nx-cache' for e in last['entries']))
    print('  end: listed',last['listed_bytes'],'room under 10GiB',a.GIB-last['listed_bytes'], 'oldest access', min(e['last_accessed_at'] for e in last['entries'])[:16])
    first=rs[0]
    print('  start',first['started_at'],'oldest access',min(e['last_accessed_at'] for e in first['entries'])[:16], 'tag entries', sorted({e['ref'][-8:] for e in first['entries'] if a.scope(e)=='tag'}))
    # usage lag
    lag=[]; 
    for x,y in zip(rs,rs[1:]):
        pass
    mism=sum(r['usage_bytes']!=r['listed_bytes'] for r in rs); print('  readings usage!=listed',mism,'of',len(rs),'max diff MB',round(max(abs(r['usage_bytes']-r['listed_bytes']) for r in rs)/1e6))
# per-run crossing share for candidate headrooms
heads={'onepipeline':[1.07,1.27,0.238],'onetaskgraph':[0.65,6.0],'onepipeline-ui':[1.83,1.476]}
for repo,hs in heads.items():
    runs=collections.defaultdict(list)
    for e in a.warm_saves(s,repo):
        runs[e['run']].append(e)
    nsaved=[v for v in runs.values() if sum(x['bytes'] for x in v)>50e6]
    legs=[]
    for v in nsaved:
        big=[x for x in v if x['bytes']>50e6]
        legs.append(len({x['leg'] for x in big}))
    print(repo,'all warm runs',len(h.workflow_runs(s,repo,a.REPOS[repo]['warm'])),'with >50MB',len(nsaved),'legs saving big',collections.Counter(legs))
    for H in hs:
        cross=later=0
        for v in nsaved:
            v=sorted(v,key=lambda x:x['finished']); cum=0; crossed=None
            for i,x in enumerate(v):
                cum+=x['bytes']
                if cum>H*1e9 and crossed is None: crossed=i
            if crossed is not None:
                cross+=1
                if crossed<len(v)-1: later+=1
        print('   H',H,'crossing runs',cross,'of',len(nsaved),'with a later save in the same run',later)
for repo in a.REPOS:
    prs=h.merged_prs(s,repo); ci=a.final_ci(h.workflow_runs(s,repo,'ci.yml'))
    def moved(p,last=False):
        run=ci.get(p['headRefOid'])
        return run and [o for o in prs if run['created_at']<o['mergedAt']<p['mergedAt']]
    for label,subset in (('last40',prs[-40:]),('all',prs)):
        ord=[p for p in subset if not a.is_release(p)]
        m=[p for p in ord if moved(p)]
        print(repo,label,'ordinary',len(ord),'moved',len(m),'moved by non-release',len([p for p in ord if any(not a.is_release(o) for o in (moved(p) or []))]))
    # gap between final CI start and merge
    gaps=[a.minutes(p['mergedAt'],ci[p['headRefOid']]['created_at']) for p in prs if not a.is_release(p) and p['headRefOid'] in ci]
    print('  final CI start to merge min', a.summary(gaps))
    # base oid: merged PR base != ?
    print('  rate/day', len(prs)/30)
