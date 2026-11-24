import json,os,sys,collections
S=os.environ['ONEPIPELINE_NODE_SCRATCH_DIR']
F=json.load(open(f'{S}/final.json'))['floor']
G=json.load(open(f'{S}/git-counts.json'))  # engine git per test, measuring host
move=set(open(f'{S}/moveset.txt').read().split())
res={}
for p in ['windows','macos','linux']:
  P=json.load(open(f'{S}/r2-split-{p}.json'))[f'{p}-tier']
  T=P['tests']; fl=F[p]['out']/1000
  a=collections.Counter(); mv=collections.Counter()
  for t,x in T.items():
    c=x['categories']; fg=x['fg']
    os_=fg.get('os_start',0)+fg.get('os_exit',0); pre=fg.get('preamble_startup',0)+fg.get('preamble_registry',0)
    eng=fg.get('engine_own',0)+fg.get('engine_git',0)+fg.get('engine_child_other',0)+x['bg_spawn']
    wait=c['wait']+x['bg_wait']
    n=G.get(t.replace('::','.'),(0,0))[0]; eg=n*fl
    fgit=min(eg,eng); bgit=min(eg-fgit,wait)
    row={'wall':x['wall'],'os':os_,'pre':pre,'egit_fg':fgit,'egit_bg':bgit,'engine_rest':eng-fgit,'wait_rest':wait-bgit,'hgit':c['git'],'fixture':c['fixture'],'unattr':x['unattributed'],
         'other':x['spawn_other']+c['build']+c['fs']+x['unpaired_fg']+x['op_residual'],'gits':n,'egit_unplaced':eg-fgit-bgit}
    a.update(row)
    if t in move: mv.update(row)
  # registry compile tier-wide, foreground
  comp=sum(x['fg_spans'].get('registry:events:compile',0) for x in T.values())
  reg=sum(sum(v for k,v in x['fg_spans'].items() if k.startswith('registry:events')) for x in T.values())
  land=sum(sum(v for k,v in x['fg_spans'].items() if k.startswith('onevcs:landing_status')) for x in T.values())
  res[p]={'tests':len(T),'span':P['tier_span'],'conc':P['concurrency'],'all':dict(a),'move':dict(mv),'registry':reg,'compile':comp,'landing':land,'fg_n':sum(x['fg_n'] for x in T.values()),'floor_ms':F[p]['out']}
  print('##',p,len(T),'tests summed',round(a['wall']),'span',round(P['tier_span']),'conc',round(P['concurrency'],2),'fg procs',res[p]['fg_n'])
  for k in ['os','pre','egit_fg','egit_bg','egit_unplaced','engine_rest','wait_rest','hgit','fixture','unattr','other','gits']:
    print(f'   {k:14} all {a[k]:9.1f}  moveset {mv[k]:8.1f}  rest {a[k]-mv[k]:9.1f}')
  print('   registry',round(reg,1),'compile',round(comp,1),'landing',round(land,1))
json.dump(res,open(f'{S}/tierwide.json','w'))
