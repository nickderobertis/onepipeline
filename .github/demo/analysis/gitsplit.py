"""Per test of a serial run: how many git processes ran, and their own time (trace2 atexit t_abs).
usage: gitsplit.py TRACE2.jsonl RECORDS_DIR  -> json on stdout"""
import json,sys,os,datetime,collections
sys.path.insert(0,os.path.dirname(__file__)); import tier
trace,d=sys.argv[1:3]
R=tier.load(d)
wins=sorted((r['started_at'],r['started_at']+r['wall_s'],t) for t,r in R.items())
start={}; out=collections.defaultdict(lambda:{'n':0,'self_s':0.0}); stray=0
def ts(s):
  return datetime.datetime.strptime(s.replace('Z',''),'%Y-%m-%dT%H:%M:%S.%f').replace(tzinfo=datetime.timezone.utc).timestamp()
for l in open(trace,errors='replace'):
  try: e=json.loads(l)
  except ValueError: continue
  if e.get('event')=='start': start[e['sid']]=ts(e['time'])
  elif e.get('event')=='atexit' and e['sid'] in start:
    t0=start.pop(e['sid'])
    for a,b,t in wins:
      if a-0.05<=t0<=b+0.05:
        out[t]['n']+=1; out[t]['self_s']+=e['t_abs']; break
    else: stray+=1
json.dump({'tests':out,'stray':stray},sys.stdout)
