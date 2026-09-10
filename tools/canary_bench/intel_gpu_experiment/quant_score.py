from pathlib import Path
import json,re
r=Path('/tmp/canary-vulkan-research');manifest={x['case']:x for x in json.loads((r/'human/manifest.json').read_text())}
def words(t):return re.findall(r"[a-z0-9]+(?:'[a-z0-9]+)?",t.lower())
def distance(a,b):
 prev=list(range(len(b)+1))
 for i,x in enumerate(a,1):
  cur=[i]
  for j,y in enumerate(b,1):cur.append(min(cur[-1]+1,prev[j]+1,prev[j-1]+(x!=y)))
  prev=cur
 return prev[-1]
result={}
for name in ['cpu','hybrid','q4-decoder','q4-all']:
 p=r/f'quant-validation-0-{name}.jsonl'
 if not p.exists():continue
 rows=[x for x in map(json.loads,p.read_text().splitlines()) if x.get('event')=='transcribed' and x['round']==1]
 cases=[]
 for x in rows:
  case=Path(x['audio']).stem;ref=manifest[case]['reference'];a,b=words(ref),words(x['text'] or '')
  cases.append({'case':case,'seconds':x['seconds'],'reference':ref,'text':x['text'],'error':x['error'],'word_errors':distance(a,b),'reference_words':len(a)})
 errors=sum(x['word_errors'] for x in cases);n=sum(x['reference_words'] for x in cases)
 result[name]={'seconds':sum(x['seconds'] for x in cases),'word_errors':errors,'reference_words':n,'wer':errors/n,'cases':cases}
(r/'quant-summary.json').write_text(json.dumps(result,indent=2)+'\n')
for k,v in result.items():print(k,{x:y for x,y in v.items() if x!='cases'})
