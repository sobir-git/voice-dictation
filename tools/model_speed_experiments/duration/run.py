import os,json,time,subprocess,statistics,re
from pathlib import Path
r=Path(os.environ.get('DURATION_BENCH_DIR',str(Path(__file__).resolve().parent)))
h=Path.home()/'.cache/huggingface/hub'
models={'canary':next(h.glob('models--handy-computer--canary-180m-flash-gguf/snapshots/*/*.gguf')),'parakeet':next(h.glob('models--handy-computer--parakeet-unified-en-0.6b-gguf/snapshots/*/*.gguf')),'whisper':next(h.glob('models--Systran--faster-whisper-base.en/snapshots/*/model.bin')).parent}
results=[]
for model in models:
 for duration in [2,5,15,30,60]:
  pair=[]
  for variant in ['baseline','candidate']:
   env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_'))}
   env.update(BENCH_SKIP_IDLE='1',BENCH_THREADS='8',BENCH_BACKEND='cpu')
   affinity=False
   if model=='whisper':
    cmd=['/home/fire/projects/voice-dictation/target/release/whisper-speed-bench',str(models[model]),'3',str(r/f'{duration}s.f32')]
    env['BENCH_THREADS']='4'
    if variant=='candidate':
     env.update(BENCH_THREADS='8',BENCH_CUSTOM='1',BENCH_FAST_MEL='1',BENCH_PAD_SECONDS='5',BENCH_MIN_SECONDS='10',BENCH_SHORT_ONLY='1');affinity=True
   else:
    cmd=['/tmp/canary-vulkan-research/target/release/canary-backend-bench','gguf',str(models[model]),'3',str(r/f'{duration}s.f32')]
    if model=='parakeet':env['BENCH_STREAM']='1'
    if variant=='candidate':
     env['BENCH_BACKEND']='vulkan'
     if model=='canary':env.update(CANARY_HYBRID='1',CANARY_DIRECT_PRE_DW='1',GGML_VK_DISABLE_F16='1',OMP_WAIT_POLICY='ACTIVE',BENCH_PAUSE_OMP='1');affinity=True
   if affinity:cmd=['taskset','-c','0-7',*cmd]
   name=f'{model}-{duration}-{variant}';start=time.monotonic();gpu_peak=0;rss_peak=0
   with (r/f'{name}.jsonl').open('w') as out,(r/f'{name}.stderr').open('w') as err:
    p=subprocess.Popen(cmd,env=env,stdout=out,stderr=err)
    while p.poll() is None:
     try:
      status=Path(f'/proc/{p.pid}/status').read_text();m=re.search(r'VmHWM:\s+(\d+)',status)
      if m:rss_peak=max(rss_peak,int(m[1])/1024)
      clients={}
      for fd in Path(f'/proc/{p.pid}/fdinfo').glob('*'):
       try:s=fd.read_text()
       except OSError:continue
       client=re.search(r'drm-client-id:\s+(\d+)',s);resident=re.search(r'drm-resident-system0:\s+(\d+)\s*(\w*)',s)
       if client and resident:clients[client[1]]=int(resident[1])*{'KiB':1024,'MiB':1048576}.get(resident[2],1)
      gpu_peak=max(gpu_peak,sum(clients.values())/1048576)
     except OSError:pass
     if time.monotonic()-start>300:p.kill();raise RuntimeError(name+' timeout')
     time.sleep(.1)
   if p.returncode:raise RuntimeError(name+' failed '+(r/f'{name}.stderr').read_text()[-1000:])
   rows=[x for x in map(json.loads,(r/f'{name}.jsonl').read_text().splitlines()) if x.get('event')=='transcribed']
   assert len(rows)==3 and all(not x.get('error') for x in rows)
   row={'model':model,'duration_s':duration,'variant':variant,'warm_seconds':statistics.median(x['seconds'] for x in rows[1:]),'warm_samples_s':[x['seconds'] for x in rows[1:]],'peak_process_mib':max(rss_peak,max(x['peak_rss_kib']/1024 for x in rows)),'sampled_peak_gpu_resident_mib':gpu_peak,'text':rows[-1]['text'],'command':cmd,'flags':{k:v for k,v in env.items() if k.startswith(('BENCH_','CANARY_','GGML_','OMP_'))}}
   results.append(row);pair.append(row);(r/'results.json').write_text(json.dumps(results,indent=2));print(name,round(row['warm_seconds'],3),'sec RSS',round(row['peak_process_mib'],1),'GPU',round(gpu_peak,1),flush=True)
  norm=lambda s:re.findall(r'\w+',s.lower())
  print('PAIR',model,duration,'speedup',round(pair[0]['warm_seconds']/pair[1]['warm_seconds'],3),'word_match',norm(pair[0]['text'])==norm(pair[1]['text']),flush=True)
