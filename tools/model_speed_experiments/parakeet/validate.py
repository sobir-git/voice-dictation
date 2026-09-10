from pathlib import Path
import os,subprocess,json,statistics
r=Path('/tmp/parakeet-research');model='/home/fire/.cache/huggingface/hub/models--handy-computer--parakeet-unified-en-0.6b-gguf/snapshots/7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf';clips=[Path('/tmp/canary-vulkan-research/five-second-screen.f32'),*sorted(Path('/tmp/canary-vulkan-research/validation-five').glob('*.f32'))[:4]]
variants={'cpu':{'BENCH_BACKEND':'cpu'},'gpu':{'BENCH_BACKEND':'vulkan'},'gpu-f32':{'BENCH_BACKEND':'vulkan','GGML_VK_DISABLE_F16':'1'}};results={n:[] for n in variants}
for cycle in range(3):
 names=list(variants);names=names[cycle:]+names[:cycle]
 for name in names:
  env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_'))};env.update(variants[name],BENCH_STREAM='1',BENCH_THREADS='8')
  p=subprocess.run(['/tmp/canary-vulkan-research/target/release/canary-backend-bench','gguf',model,'2',*map(str,clips)],env=env,capture_output=True,text=True,timeout=25)
  (r/f'validation-{cycle}-{name}.jsonl').write_text(p.stdout);(r/f'validation-{cycle}-{name}.stderr').write_text(p.stderr);p.check_returncode();rows=list(map(json.loads,p.stdout.splitlines()));results[name].extend(rows)
  print(cycle,name,sum(x['seconds'] for x in rows if x['event']=='transcribed' and x['round']==1),flush=True)
summary=[]
for clip in clips:
 by={n:[x for x in rows if x['event']=='transcribed' and x['audio']==str(clip) and x['round']==1] for n,rows in results.items()};med={n:statistics.median(x['seconds'] for x in v) for n,v in by.items()};texts={n:sorted(set(x['text'] for x in v)) for n,v in by.items()};summary.append({'audio':clip.name,'medians':med,'texts':texts,'texts_match':len(set(t for v in texts.values() for t in v))==1})
totals={n:sum(x['medians'][n] for x in summary) for n in variants};out={'total_medians':totals,'speedup':{n:totals['cpu']/v for n,v in totals.items()},'all_texts_match':all(x['texts_match'] for x in summary),'clips':summary,'idle':{n:[x for x in v if x['event']=='idle'] for n,v in results.items()}}
(r/'validation-summary.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps({k:v for k,v in out.items() if k!='clips'},indent=2))
