from pathlib import Path
import gguf,numpy as np,json,sys
src=Path('/home/fire/.cache/huggingface/hub/models--handy-computer--canary-180m-flash-gguf/snapshots/b147f9dc52b59f0998e410540a84727bd86457fd/canary-180m-flash-Q8_0.gguf')
r=gguf.GGUFReader(src)
for mode in ('decoder','all'):
 out=Path('/tmp/canary-vulkan-research')/f'canary-q8-requantized-q4-{mode}.gguf'
 w=gguf.GGUFWriter(out,r.fields['general.architecture'].contents())
 for key,field in r.fields.items():
  if key.startswith('GGUF.') or key=='general.architecture':continue
  w.add_key_value(key,field.contents(),field.types[0],field.types[-1] if len(field.types)>1 else None)
 count=0
 for t in r.tensors:
  if t.tensor_type==gguf.GGMLQuantizationType.Q8_0 and (mode=='all' or t.name.startswith('dec.')):
   data=gguf.quantize(gguf.dequantize(t.data,t.tensor_type),gguf.GGMLQuantizationType.Q4_0)
   w.add_tensor(t.name,data,raw_dtype=gguf.GGMLQuantizationType.Q4_0);count+=1
  else:w.add_tensor(t.name,t.data,raw_dtype=t.tensor_type)
 w.write_header_to_file();w.write_kv_data_to_file();w.write_tensors_to_file();w.close()
 check=gguf.GGUFReader(out)
 assert len(check.tensors)==len(r.tensors)
 for a,b in zip(r.tensors,check.tensors):
  assert a.name==b.name and np.array_equal(a.shape,b.shape)
  if a.tensor_type==b.tensor_type:assert np.array_equal(a.data,b.data)
  else:assert np.array_equal(gguf.quantize(gguf.dequantize(a.data,a.tensor_type),gguf.GGMLQuantizationType.Q4_0),b.data)
 print(mode,count,out.stat().st_size,flush=True)
