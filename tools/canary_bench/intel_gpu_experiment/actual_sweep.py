from pathlib import Path
import subprocess,os,json,struct,time
r=Path('/tmp/canary-vulkan-research')
variants=[('base',None)]
for bm,bn,wm,wn in [(32,32,16,16),(32,64,16,16),(64,32,16,16),(32,32,32,32)]:
 for warp in [8,16,32]:
  block=warp*(bm//wm)*(bn//wn)
  if block>256:continue
  variants.append((f'b{bm}x{bn}w{wm}x{wn}s{warp}',','.join(map(str,[block,bm,bn,16,wm,wn,1,2,2,1,warp]))))
ref=None;results=[];start=time.perf_counter()
for name,tile in variants:
 env={k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_'))}
 if tile:env['CANARY_VK_FLOAT_TILE']=tile
 try: p=subprocess.run([str(r/'float-actual-probe'),str(r/f'actual-{name}.bin')],env=env,capture_output=True,text=True,timeout=8)
 except subprocess.TimeoutExpired:
  print(name,'timeout',flush=True);continue
 (r/f'actual-{name}.jsonl').write_text(p.stdout);(r/f'actual-{name}.stderr').write_text(p.stderr)
 if p.returncode:print(name,'FAILED',p.stderr[-400:],flush=True);continue
 data=(r/f'actual-{name}.bin').read_bytes();values=struct.unpack(f'{len(data)//4}f',data)
 if ref is None:ref=values
 error=max(abs(a-b) for a,b in zip(ref,values));rows=list(map(json.loads,p.stdout.splitlines()));score=sum(x['median_us'] for x in rows)
 results.append({'name':name,'tile':tile,'score_us':score,'max_abs':error,'shapes':rows})
 print(name,round(score,1),'max_abs',error,flush=True)
(r/'actual-summary.json').write_text(json.dumps({'seconds':time.perf_counter()-start,'results':results},indent=2)+'\n')
