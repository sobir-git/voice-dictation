from pathlib import Path
import subprocess,time,hashlib,json
root=Path('/tmp/canary-vulkan-research')
source=root/'shader-edit'
if not source.exists():
 import shutil
 shutil.copytree(root/'native/ggml/src/ggml-vulkan/vulkan-shaders',source)
out=root/'shader-bin';out.mkdir(exist_ok=True)
start=time.perf_counter()
for name,define in [('subgroup','USE_SUBGROUP_ADD_NO_SHMEM'),('hybrid','USE_SUBGROUP_ADD')]:
 subprocess.run([str(root/'1.4.357.1/x86_64/bin/glslc'),'--target-env=vulkan1.2','-O','-fshader-stage=compute','-I',str(source),'-DDATA_A_Q8_0=1','-DD_TYPE=float','-DFLOAT_TYPE=float','-DFLOAT_TYPEV2=vec2','-DACC_TYPE=float',f'-D{define}=1',str(source/'mul_mat_vecq.comp'),'-o',str(out/f'mmvq-{name}.spv')],check=True)
print(json.dumps({'compile_seconds':time.perf_counter()-start,'shaders':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.glob('*.spv')}}))
