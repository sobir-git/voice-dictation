from pathlib import Path
import os,subprocess,json,re
r=Path('/tmp/parakeet-research');manifest=json.loads((Path('/tmp/whisper-research/public-extra/manifest.json')).read_text());clips=[Path('/tmp/whisper-research/public-extra')/f"{x['case']}.f32" for x in manifest];model='/home/fire/.cache/huggingface/hub/models--handy-computer--parakeet-unified-en-0.6b-gguf/snapshots/7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf'
variants={'baseline':({'BENCH_BACKEND':'cpu','BENCH_THREADS':'8','BENCH_STREAM':'1','BENCH_SKIP_IDLE':'1'},None),'gpu':({'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8','BENCH_STREAM':'1','BENCH_SKIP_IDLE':'1'},None)}
def words(t):return re.findall(r"[a-z0-9]+(?:'[a-z0-9]+)?",t.lower())
def dist(a,b):
 prev=list(range(len(b)+1))
 for i,x in enumerate(a,1):
  cur=[i]
  for j,y in enumerate(b,1):cur.append(min(cur[-1]+1,prev[j]+1,prev[j-1]+(x!=y)))
  prev=cur
 return prev[-1]
result={}
for name,(extra,pin) in variants.items():
 env={k:v for k,v in os.environ.items() if not k.startswith(('BENCH_','OMP_','GOMP_','CT2_','CANARY_','GGML_VK_'))};env.update(extra)
 cmd=['/tmp/canary-vulkan-research/target/release/canary-backend-bench','gguf',model,'2',*map(str,clips)]
 if pin:cmd=['taskset','-c',pin,*cmd]
 p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=60);(r/f'quality-{name}.jsonl').write_text(p.stdout);(r/f'quality-{name}.stderr').write_text(p.stderr);p.check_returncode()
 rows=[x for x in map(json.loads,p.stdout.splitlines()) if x['event']=='transcribed' and x['round']==1];cases=[]
 for ref,x in zip(manifest,rows):
  a,b=words(ref['reference']),words(x['text']);cases.append({'case':ref['case'],'speaker_id':ref['speaker_id'],'reference':ref['reference'],'text':x['text'],'seconds':x['seconds'],'word_errors':dist(a,b),'reference_words':len(a),'peak_rss_kib':x['peak_rss_kib']})
 n=sum(x['reference_words'] for x in cases);e=sum(x['word_errors'] for x in cases);result[name]={'seconds':sum(x['seconds'] for x in cases),'word_errors':e,'reference_words':n,'wer':e/n,'cases':cases};(r/'quality-summary.json').write_text(json.dumps(result,indent=2)+'\n');print(name,{k:v for k,v in result[name].items() if k!='cases'},flush=True)
