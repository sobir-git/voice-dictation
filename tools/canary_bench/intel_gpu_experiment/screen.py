from pathlib import Path
import subprocess,json,os,time,argparse
r=Path('/tmp/canary-vulkan-research')
q8='/home/fire/.cache/huggingface/hub/models--handy-computer--canary-180m-flash-gguf/snapshots/b147f9dc52b59f0998e410540a84727bd86457fd/canary-180m-flash-Q8_0.gguf'
variants=[('cpu',q8,{}),('gpu',q8,{'BENCH_BACKEND':'vulkan','GGML_VK_FORCE_MMVQ':'1'}),('packed-tile',q8,{'BENCH_BACKEND':'vulkan','GGML_VK_FORCE_MMVQ':'1','CANARY_VK_REORDER':'1','CANARY_VK_ROWS':'8','CANARY_VK_SUBGROUP':'8','CANARY_VK_LARGE':'1','CANARY_VK_SHADER_DIR':str(r/'shader-bin')}),('f16-decoder',str(r/'canary-q8-expanded-f16-decoder.gguf'),{'BENCH_BACKEND':'vulkan'}),('f16-all',str(r/'canary-q8-expanded-f16-all.gguf'),{'BENCH_BACKEND':'vulkan'})]
variants += [('hybrid',q8,{'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8','CANARY_HYBRID':'1','CANARY_DIRECT_PRE_DW':'1','OMP_WAIT_POLICY':'ACTIVE','BENCH_PAUSE_OMP':'1'}),('hybrid-f32',q8,{'BENCH_BACKEND':'vulkan','BENCH_THREADS':'8','CANARY_HYBRID':'1','CANARY_DIRECT_PRE_DW':'1','OMP_WAIT_POLICY':'ACTIVE','BENCH_PAUSE_OMP':'1','GGML_VK_DISABLE_F16':'1'})]
parser=argparse.ArgumentParser()
parser.add_argument('--variants',default='cpu,hybrid,hybrid-f32')
parser.add_argument('--rounds',type=int,default=3)
args=parser.parse_args()
selected=set(args.variants.split(','))
for name,model,extra in variants:
 if name not in selected:continue
 env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_'))}
 env.update(BENCH_SKIP_IDLE='1',**extra)
 cmd=[str(r/'target/release/canary-backend-bench'),'gguf',model,str(args.rounds),str(r/'five-second-screen.f32')]
 if name.startswith('hybrid'):cmd=['taskset','-c','0-7',*cmd]
 start=time.perf_counter();p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=10)
 (r/f'five-second-{name}.jsonl').write_text(p.stdout);(r/f'five-second-{name}.stderr').write_text(p.stderr)
 if p.returncode:print(name,'failed',p.stderr[-2000:],flush=True);continue
 rows=[d for d in map(json.loads,p.stdout.splitlines()) if d.get('event')=='transcribed']
 print(name,'wall',time.perf_counter()-start,[d['seconds'] for d in rows],[d['error'] for d in rows],flush=True)
