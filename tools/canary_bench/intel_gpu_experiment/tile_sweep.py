from pathlib import Path
import subprocess,json,os,statistics,itertools
root=Path('/tmp/canary-vulkan-research')
model='/home/fire/.cache/huggingface/hub/models--handy-computer--canary-180m-flash-gguf/snapshots/b147f9dc52b59f0998e410540a84727bd86457fd/canary-180m-flash-Q8_0.gguf'
variants=[('baseline',{})]+[(f'r{r}s{s}l{l}',dict(CANARY_VK_ROWS=str(r),CANARY_VK_SUBGROUP=str(s),**({'CANARY_VK_LARGE':'1'} if l else {}))) for r,s,l in itertools.product((1,2,4,8),(8,16,32),(0,1))]
for name,extra in variants:
 env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_VK_','GGML_VK_'))}
 env.update(LD_LIBRARY_PATH=str(root/'deps/usr/lib/x86_64-linux-gnu'),BENCH_BACKEND='vulkan',GGML_VK_FORCE_MMVQ='1',**extra)
 p=subprocess.run([str(root/'target/release/canary-backend-bench'),'gguf',model,'3','/tmp/voice-canary-benchmark/run2/medium.f32'],env=env,capture_output=True,text=True,timeout=120)
 (root/f'tile-{name}.jsonl').write_text(p.stdout);(root/f'tile-{name}.stderr').write_text(p.stderr)
 if p.returncode:
  print(name,'FAILED',p.returncode,p.stderr[-300:],flush=True);continue
 rows=[r for r in map(json.loads,p.stdout.splitlines()) if r.get('event')=='transcribed']
 print(name,[r['seconds'] for r in rows],[r['error'] for r in rows],flush=True)
