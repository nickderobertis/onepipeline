import json,os,sys,collections
S=os.environ['ONEPIPELINE_NODE_SCRATCH_DIR']; sys.path.insert(0,S); import tier
R2=f'{S}/r2'
FLOOR={}
for p,f in [('linux','floor-target-llvm-cov-target-debug-onepipeline.json'),('macos','floor-target-debug-onepipeline.json'),('windows','floor-target-debug-onepipeline.exe.json')]:
  d=json.load(open(f'{R2}/demo-profile-{p}/{f}')); c={x['name']:x['mean_ms'] for x in d['cases']}
  FLOOR[p]={'out':c['git rev-parse --git-dir'],'in_traced':d['git_trace2_atexit']['mean_ms'],'cpu':d['cpu'].strip(),'cpus':d['cpus'],'c':c}
sample=[l.strip() for l in open('.github/demo/sample.txt') if l.strip()]
OUT={}
for p in ['windows','macos','linux']:
  d=json.load(open(f'{S}/r2-split-{p}.json')); g=json.load(open(f'{S}/git-{p}.json'))['tests']
  T=d[f'{p}-tier']['tests'];A=d[f'{p}-alone']['tests']
  adj=(FLOOR[p]['out']-FLOOR[p]['in_traced'])/1000
  rows={}
  for t in sample:
    for lab,X in (('alone',A[t]),('tier',T[t])):
      c=X['categories']; fg=X['fg']
      startup=fg.get('os_start',0)+fg.get('os_exit',0)+fg.get('preamble_startup',0)+fg.get('preamble_registry',0)
      engine=fg.get('engine_own',0)+fg.get('engine_git',0)+fg.get('engine_child_other',0)+X['bg_spawn']
      wait=c['wait']+X['bg_wait']
      other=X['spawn_other']+c['build']+c['fs']+X['unpaired_fg']+X['op_residual']
      row={'wall':X['wall'],'startup':startup,'engine':engine,'hgit':c['git'],'fixture':c['fixture'],'wait':wait,'other':other,'unattr':X['unattributed'],'fg_n':X['fg_n'],'op_n':X['onepipeline_n']}
      if lab=='alone':
        gt=g[t]; total_git=gt['self_s']+gt['n']*adj
        eg=max(0.0,total_git-c['git'])
        row['git_n']=gt['n']; row['egit_est']=eg; row['egit']=min(eg,engine); row['engine_rest']=engine-row['egit']
      rows.setdefault(t,{})[lab]=row
  OUT[p]={'rows':rows,'adj_ms':adj*1000}
json.dump({'floor':FLOOR,'out':OUT},open(f'{S}/final.json','w'))
for p in OUT:
  print('##',p,FLOOR[p]['cpu'],'adj ms/git',round(OUT[p]['adj_ms'],2))
  tot=collections.Counter()
  for t,r in OUT[p]['rows'].items():
    a=r['alone'];x=r['tier']
    for k in ['wall','startup','egit','engine_rest','hgit','fixture','wait','other','unattr']: tot[k]+=a[k]
    tot['tier']+=x['wall']
    print(f"{t[:48]:48} tier {x['wall']:6.1f} alone {a['wall']:6.1f} | st {a['startup']:4.1f} egit {a['egit']:5.1f}({a['egit_est']:5.1f}) eng {a['engine_rest']:5.1f} hgit {a['hgit']:4.1f} fx {a['fixture']:3.1f} wait {a['wait']:5.1f} oth {a['other']:4.1f} un {a['unattr']:5.1f} gits {a['git_n']}")
  print({k:round(v,1) for k,v in tot.items()})
