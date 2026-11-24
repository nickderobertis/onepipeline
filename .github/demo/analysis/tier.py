import json,glob,sys,collections
def load(d):
  runs=collections.defaultdict(list)
  for f in glob.glob(f'{d}/*.json'):
    r=json.load(open(f)); runs[r['run_id']].append(r)
  run=max(runs,key=lambda k:len(runs[k]))
  return {r['test']:r for r in runs[run]}
def concurrency(recs):
  ev=[]
  for r in recs.values(): ev+= [(r['started_at'],1),(r['started_at']+r['wall_s'],-1)]
  ev.sort(); n=0;t=None;hist=collections.Counter()
  for x,d in ev:
    if t is not None: hist[n]+=x-t
    n+=d;t=x
  span=ev[-1][0]-ev[0][0]
  busy=sum(v*k for k,v in hist.items())
  return span,busy/span,hist
if __name__=='__main__':
  for lab,d in [a.split('=') for a in sys.argv[1:]]:
    R=load(d); span,mean,hist=concurrency(R)
    tot=sum(r['wall_s'] for r in R.values())
    print(f'{lab}: tests {len(R)} summed {tot:.1f} span {span:.1f} mean-concurrency {mean:.2f} (summed/span {tot/span:.2f})')
    print('   ', {k:round(v,1) for k,v in sorted(hist.items())})
