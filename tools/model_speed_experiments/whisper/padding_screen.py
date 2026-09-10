from pathlib import Path
import subprocess,os,json
r=Path('/tmp/whisper-research');binary='/home/fire/projects/voice-dictation/target/release/whisper-speed-bench';model='/home/fire/.cache/huggingface/hub/models--Systran--faster-whisper-base.en/snapshots/3d3d5dee26484f91867d81cb899cfcf72b96be6c';clip='/tmp/canary-vulkan-research/five-second-screen.f32'
variants=[('original',{},'0-7'),('custom-full',{'BENCH_CUSTOM':'1'},'0-7'),('pad1',{'BENCH_CUSTOM':'1','BENCH_PAD_SECONDS':'1'},'0-7'),('pad3',{'BENCH_CUSTOM':'1','BENCH_PAD_SECONDS':'3'},'0-7'),('pad5',{'BENCH_CUSTOM':'1','BENCH_PAD_SECONDS':'5'},'0-7')]
for name,extra,affinity in variants:
 env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_','CT2_'))};env.update(BENCH_THREADS="8",**extra)
 cmd=[binary,model,'3',clip]
 if affinity:cmd=['taskset','-c',affinity,*cmd]
 try:p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=20)
 except subprocess.TimeoutExpired:print(name,'timeout',flush=True);continue
 (r/f'{name}.jsonl').write_text(p.stdout);(r/f'{name}.stderr').write_text(p.stderr)
 if p.returncode:print(name,'failed',p.stderr[-600:],flush=True);continue
 rows=[x for x in map(json.loads,p.stdout.splitlines()) if x.get('event')=='transcribed'];print(name,[round(x['seconds'],4) for x in rows],[x['text'] for x in rows],flush=True)
