from pathlib import Path
import os,subprocess,json,time,statistics
r=Path('/tmp/canary-vulkan-research')
model='/home/fire/.cache/huggingface/hub/models--handy-computer--canary-180m-flash-gguf/snapshots/b147f9dc52b59f0998e410540a84727bd86457fd/canary-180m-flash-Q8_0.gguf'
manifest=[x for x in json.loads((r/'human/manifest.json').read_text()) if x['seconds']<=5][:12]
clips=[r/'human'/f"{x['case']}.f32" for x in manifest]
common={'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8','CANARY_HYBRID':'1','CANARY_DIRECT_PRE_DW':'1','OMP_WAIT_POLICY':'ACTIVE','BENCH_PAUSE_OMP':'1'}
variants={'cpu':{'BENCH_BACKEND':'cpu','BENCH_THREADS':'8'},'hybrid':common,'q4-decoder':common,'q4-all':common}
results={}
for cycle in range(1):
 for name in list(variants):
  if time.time()+15>json.loads((r/'deadline.json').read_text())['deadline_epoch']:raise SystemExit('Deadline reached')
  env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_'))};env.update(variants[name]);env['BENCH_SKIP_IDLE']='1'
  cmd=[str(r/'target/release/canary-backend-bench'),'gguf',(str(r/f'canary-q8-requantized-q4-{name[3:]}.gguf') if name.startswith('q4-') else model),'2',*map(str,clips)]
  if name!='cpu':cmd=['taskset','-c','0-7',*cmd]
  p=subprocess.run(cmd,env=env,text=True,capture_output=True,timeout=15)
  (r/f'quant-validation-{cycle}-{name}.jsonl').write_text(p.stdout);(r/f'quant-validation-{cycle}-{name}.stderr').write_text(p.stderr)
  p.check_returncode();rows=[x for x in map(json.loads,p.stdout.splitlines()) if x.get('event')=='transcribed'];results[f'{cycle}-{name}']=rows
  print(cycle,name,round(sum(x['seconds'] for x in rows if x['round']==1),4),flush=True)
