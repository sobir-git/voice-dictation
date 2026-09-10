from pathlib import Path
import subprocess,os,json
r=Path('/tmp/parakeet-research');model='/home/fire/.cache/huggingface/hub/models--handy-computer--canary-180m-flash-gguf/snapshots/b147f9dc52b59f0998e410540a84727bd86457fd/canary-180m-flash-Q8_0.gguf'
for name,flags in [('cpu',{'BENCH_BACKEND':'cpu','BENCH_THREADS':'8'}),('hybrid',{'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8','CANARY_HYBRID':'1','CANARY_DIRECT_PRE_DW':'1','GGML_VK_DISABLE_F16':'1','OMP_WAIT_POLICY':'ACTIVE','BENCH_PAUSE_OMP':'1'})]:
 env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_'))};env.update(flags,BENCH_SKIP_IDLE='1')
 cmd=['/tmp/canary-vulkan-research/target/release/canary-backend-bench','gguf',model,'2','/tmp/canary-vulkan-research/five-second-screen.f32']
 if name=='hybrid':cmd=['taskset','-c','0-7',*cmd]
 p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=15);p.check_returncode();(r/f'canary-memory-{name}.jsonl').write_text(p.stdout);(r/f'canary-memory-{name}.stderr').write_text(p.stderr)
 row=[x for x in map(json.loads,p.stdout.splitlines()) if x['event']=='transcribed'][-1]
 print(name,'peak RSS',row['peak_rss_kib']/1024,'PSS',row['smaps_kib']['Pss']/1024, 'GPU',row['drm_fdinfo'])
