import json,os,sys,collections
S=os.environ['ONEPIPELINE_NODE_SCRATCH_DIR']
F=json.load(open(f'{S}/final.json'))['floor']
sample=[l.strip() for l in open(f'{S}/sample.txt') if l.strip()]
def piece(x,n_total):
  c=x['categories']; fg=x['fg']
  st=fg.get('os_start',0)+fg.get('os_exit',0)+fg.get('preamble_startup',0)+fg.get('preamble_registry',0)
  eng=fg.get('engine_own',0)+fg.get('engine_git',0)+fg.get('engine_child_other',0)+x['bg_spawn']
  wait=c['wait']+x['bg_wait']
  return st,eng,wait,c
out={}
for p in ['windows','macos','linux']:
  d=json.load(open(f'{S}/r2-split-{p}.json')); g=json.load(open(f'{S}/git-{p}.json'))['tests']
  T=d[f'{p}-tier']['tests'];A=d[f'{p}-alone']['tests']; fl=F[p]['out']/1000
  # harness git counts from the alone records
  import glob
  hg={}
  for t in sample:
    r=json.load(open(f"{S}/r2/demo-profile-{p}/sample-alone/e2e/{t.replace('::','.')}.json"))
    hg[t]=r['programs'].get('git',{'count':0})['count']
  lines=[f"| journey | tier | alone | contention | start-up | engine git ≥ | engine other | harness git | fixture | waiting | unattributed | other | git processes |","| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
  tot=collections.Counter()
  for t in sample:
    a=A[t]; x=T[t]; st,eng,wait,c=piece(a,0)
    n=g[t]['n']; ne=max(0,n-hg[t]); eg=ne*fl; fgit=min(eg,eng); bgit=min(eg-fgit,wait)
    other=a['spawn_other']+c['build']+c['fs']+a['unpaired_fg']+a['op_residual']
    row=dict(tier=x['wall'],alone=a['wall'],cont=x['wall']-a['wall'],st=st,eg=fgit+bgit,eo=eng-fgit,hg=c['git'],fx=c['fixture'],wt=wait-bgit,un=a['unattributed'],ot=other,n=n)
    tot.update(row)
    chk=row['st']+row['eg']+row['eo']+row['hg']+row['fx']+row['wt']+row['un']+row['ot']-a['wall']
    assert abs(chk)<0.05,(t,chk)
    m,_,name=t.partition('::')
    lines.append(f"| `{m}::{name[:38]}{'…' if len(name)>38 else ''}` | {row['tier']:.1f} | {row['alone']:.1f} | {row['cont']:+.1f} | {row['st']:.1f} | {row['eg']:.1f} | {row['eo']:.1f} | {row['hg']:.1f} | {row['fx']:.1f} | {row['wt']:.1f} | {row['un']:.1f} | {row['ot']:.1f} | {n:,} |")
  lines.append(f"| **sum** | **{tot['tier']:.1f}** | **{tot['alone']:.1f}** | **{tot['cont']:+.1f}** | {tot['st']:.1f} | {tot['eg']:.1f} | {tot['eo']:.1f} | {tot['hg']:.1f} | {tot['fx']:.1f} | {tot['wt']:.1f} | {tot['un']:.1f} | {tot['ot']:.1f} | {int(tot['n']):,} |")
  out[p]='\n'.join(lines); print('##',p); print(out[p]); print(dict((k,round(v,1)) for k,v in tot.items()))
json.dump(out,open(f'{S}/tables.json','w'))
