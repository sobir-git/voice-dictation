from pathlib import Path
import subprocess,os,json,statistics
r=Path('/tmp/parakeet-research');binary=Path('/tmp/canary-vulkan-research/target/release/canary-backend-bench');clip='/tmp/canary-vulkan-research/five-second-screen.f32'
parakeet='/home/fire/.cache/huggingface/hub/models--handy-computer--parakeet-unified-en-0.6b-gguf/snapshots/7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf'
variants=[('cpu8',{'BENCH_BACKEND':'cpu','BENCH_THREADS':'8'},None),('cpu4p',{'BENCH_BACKEND':'cpu','BENCH_THREADS':'4'},'0,2,4,6'),('cpu8p',{'BENCH_BACKEND':'cpu','BENCH_THREADS':'8'},'0-7'),('cpu8p-active',{'BENCH_BACKEND':'cpu','BENCH_THREADS':'8','OMP_WAIT_POLICY':'ACTIVE','BENCH_PAUSE_OMP':'1'},'0-7'),('vulkan',{'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8'},None),('vulkan-f32',{'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8','GGML_VK_DISABLE_F16':'1'},None)]
for name,extra,affinity in variants:
 env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_'))};env.update(extra,BENCH_SKIP_IDLE='1',BENCH_STREAM='1')
 cmd=[str(binary),'gguf',parakeet,'3',clip]
 if affinity:cmd=['taskset','-c',affinity,*cmd]
 try:p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=20)
 except subprocess.TimeoutExpired as e:
  (r/f'{name}.timeout').write_text(str(e));print(name,'timeout',flush=True);continue
 (r/f'{name}.jsonl').write_text(p.stdout);(r/f'{name}.stderr').write_text(p.stderr)
 if p.returncode:print(name,'failed',p.stderr[-600:],flush=True);continue
 rows=[x for x in map(json.loads,p.stdout.splitlines()) if x.get('event')=='transcribed']
 print(name,[round(x['seconds'],4) for x in rows],[x['text'] for x in rows],flush=True)
