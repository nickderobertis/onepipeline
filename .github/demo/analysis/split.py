"""Split each test's wall into causes, from the harness record and its span records.

usage: split.py OUT.json LABEL=DIR [...]   (DIR holds <module>.<test>.json and .spans/)
"""
import json,glob,os,sys,collections
sys.path.insert(0,os.path.dirname(__file__)); import tier

def classify(name):
  if name=='startup': return 'preamble_startup'
  if name.startswith('registry:events'): return 'preamble_registry'
  if name=='child:git': return 'engine_git'
  if name.startswith('child:'): return 'engine_child_other'
  if name.startswith('wait:'): return None   # served on other threads: never self time
  return 'engine_own'

def split_test(r, spans_dir, logdir=None):
  c={k:v['s'] for k,v in r['categories'].items()}
  out=collections.OrderedDict()
  op=r['programs'].get('onepipeline',{'s':0.0,'count':0})['s']
  H={}; lines=[]
  log=f'{spans_dir}/spawns.jsonl'
  if logdir: log=os.path.join(logdir,os.path.basename(spans_dir)+'.jsonl')
  if os.path.isfile(log):
    for l in open(log):
      try: j=json.loads(l)
      except ValueError: continue
      lines.append(j); H.setdefault(j['id'],[]).append(j)
  P={}
  for f in glob.glob(f'{spans_dir}/*.json'):
    try: s=json.load(open(f))
    except ValueError: continue
    P.setdefault(s.get('spawn_id'),[]).append(s)
  cpu=collections.Counter(); verbs=collections.Counter(); verbn=collections.Counter()
  fg=collections.Counter(); spans_tot=collections.Counter(); n_fg=0; n_bg=0
  bg_spawn=bg_wait=0.0; unpaired_fg=0.0
  for j in lines:
    if j['role']!='test': continue
    if j['kind'] in ('output','status'):
      procs=P.get(j['id'])
      if not procs: unpaired_fg+=j['s']; continue
      pr=procs[0]; n_fg+=1
      own=pr['wall_ms']/1000; pre=max(0.0,pr['started_at']-j['t0']); post=j['s']-pre-own
      fg['os_start']+=pre; fg['os_exit']+=post
      if pr.get('user_cpu_ms') is not None: cpu['cpu']+=(pr['user_cpu_ms']+pr['system_cpu_ms'])/1000; cpu['child_cpu']+=((pr.get('children_user_cpu_ms') or 0)+(pr.get('children_system_cpu_ms') or 0))/1000
      verbs[pr['verb']]+=own; verbn[pr['verb']]+=1
      for sp in pr['spans']:
        k=classify(sp['name'])
        if k: fg[k]+=sp['self_wall_ms']/1000; spans_tot[sp['name']]+=sp['self_wall_ms']/1000
      fg['engine_own']+=own-sum(sp['self_wall_ms'] for sp in pr['spans'] if classify(sp['name']))/1000
    elif j['kind']=='spawn': bg_spawn+=j['s']; n_bg+=1
    elif j['kind']=='wait': bg_wait+=j['s']
  paired=sum(fg.values())
  # every span record, foreground or not: engine processes' own time tier-wide
  allp=collections.Counter(); nproc=0; procwall=0.0
  for lst in P.values():
    for pr in lst:
      nproc+=1; procwall+=pr['wall_ms']/1000
      for sp in pr['spans']:
        k=classify(sp['name'])
        if k: allp[k]+=sp['self_wall_ms']/1000
      allp['engine_own']+=pr['wall_ms']/1000-sum(sp['self_wall_ms'] for sp in pr['spans'] if classify(sp['name']))/1000
  return {
    'wall':r['wall_s'],'categories':c,'unattributed':r['unattributed_s'],
    'onepipeline_s':op,'onepipeline_n':r['programs'].get('onepipeline',{'count':0})['count'],
    'fg':dict(fg),'fg_n':n_fg,'bg_spawn':bg_spawn,'bg_n':n_bg,'bg_wait':bg_wait,'unpaired_fg':unpaired_fg,
    'op_residual':op-paired-bg_spawn-bg_wait-unpaired_fg,
    'spawn_other':c['spawn']-op,
    'all_procs':{'n':nproc,'wall':procwall,**allp},
    'fg_spans':dict(spans_tot),'fg_cpu':dict(cpu),'fg_verbs':dict(verbs),'fg_verbn':dict(verbn),
  }

def platform(d,logdir=None):
  R=tier.load(d); span,mean,hist=tier.concurrency(R)
  tests={}
  for t,r in R.items():
    sd=os.path.join(d, t.replace('::','.')+'.spans')
    tests[t]=split_test(r,sd,logdir)
  return {'tier_span':span,'concurrency':mean,'hist':{str(k):v for k,v in hist.items()},'tests':tests}

if __name__=='__main__':
  out={}
  for a in sys.argv[2:]:
    lab,rest=a.split('=',1); d,_,logdir=rest.partition(',')
    out[lab]=platform(d,logdir or None)
  json.dump(out,open(sys.argv[1],'w'))
