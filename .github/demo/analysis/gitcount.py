"""Engine git per test from a job's git-tier/<module>.<test>.spans.git.txt files (GIT_TRACE2, brief).
usage: gitcount.py DIR OUT.json"""
import os,sys,glob,json,re,collections
d,out=sys.argv[1:3]; res={}; tot=collections.Counter()
for f in glob.glob(f'{d}/*.git.txt'):
  t=os.path.basename(f)[:-len('.spans.git.txt')]
  n=x=0; el=0.0; odd=0
  for l in open(f,errors='replace'):
    if l.startswith('start '): n+=1
    elif l.startswith('atexit '):
      x+=1; m=re.search(r'elapsed:([\d.]+)',l); el+=float(m.group(1)) if m else 0
    elif not re.match(r'^(version|cmd_ancestry|cmd_name|exit|child_|exec|error|def_|alias|signal|region|data|thread|timer|counter|too_many|cmd_mode|printf|[a-z_]+\[)',l): odd+=1
  res[t]=(n,el); tot['start']+=n; tot['atexit']+=x; tot['odd']+=odd; tot['tests']+=1
json.dump(res,open(out,'w')); print(dict(tot))
