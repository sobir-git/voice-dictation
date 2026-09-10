from pathlib import Path
import time,json,sys,numpy as np,openvino as ov,onnxruntime as ort
r=Path('/tmp/voice-canary-benchmark/models/canary-180m-flash');device=sys.argv[1]
audio=np.fromfile('/tmp/canary-vulkan-research/five-second-screen.f32',dtype='<f4')[None,:]
opts=ort.SessionOptions();opts.intra_op_num_threads=8;opts.inter_op_num_threads=1;opts.add_session_config_entry("session.intra_op.allow_spinning","0");opts.add_session_config_entry("session.inter_op.allow_spinning","0")
pre=ort.InferenceSession(str(r/'nemo128.onnx'),sess_options=opts,providers=['CPUExecutionProvider'])
core=ov.Core();core.set_property({'CACHE_DIR':'/tmp/canary-openvino-research/model-cache'})
start=time.perf_counter();props={'PERFORMANCE_HINT':'LATENCY'}
if device=='CPU':props.update(INFERENCE_NUM_THREADS=8)
print(json.dumps({'event':'compiling','device':device}),flush=True)
encoder=core.compile_model(core.read_model(r/'encoder-model.int8.onnx'),device,props)
print(json.dumps({'event':'encoder_compiled','seconds':time.perf_counter()-start}),flush=True)
decoder=core.compile_model(core.read_model(r/'decoder-model.int8.onnx'),device,props)
print(json.dumps({'event':'loaded','seconds':time.perf_counter()-start}),flush=True)
vocab={};reverse={}
for line in (r/'vocab.txt').read_text().splitlines():
 if line.strip():
  token,n=line.rsplit(' ',1);vocab[token]=int(n);reverse[int(n)]=token
prompt=[vocab[x] for x in ['<|startofcontext|>','<|startoftranscript|>','<|emo:undefined|>','<|en|>','<|en|>','<|pnc|>','<|noitn|>','<|notimestamp|>','<|nodiarize|>']]
for run in range(3):
 start=time.perf_counter();f=pre.run(None,{'waveforms':audio,'waveforms_lens':np.array([80000],dtype=np.int64)})
 preout=dict(zip([x.name for x in pre.get_outputs()],f));mel_end=time.perf_counter()
 out=encoder({'audio_signal':preout['features'],'length':preout['features_lens'].astype(np.int64)});emb=out[encoder.output('encoder_embeddings')];mask=out[encoder.output('encoder_mask')];enc_end=time.perf_counter();print(json.dumps({"event":"encoded","round":run,"encoder_ms":(enc_end-mel_end)*1000}),flush=True)
 mem=np.zeros((6,1,0,1024),np.float32);ids=prompt[:];eos=False
 for step in range(256):
  out=decoder({'input_ids':np.array([ids if step==0 else ids[-1:]],dtype=np.int64),'encoder_embeddings':emb,'encoder_mask':mask,'decoder_mems':mem})
  logits=out[decoder.output('logits')];token=int(logits[0,-1].argmax())
  if token==vocab['<|endoftext|>']:eos=True;break
  ids.append(token);mem=out[decoder.output('decoder_hidden_states')]
 end=time.perf_counter();text=''.join(reverse[t].replace('▁',' ') for t in ids if not reverse[t].startswith('<|')).strip()
 print(json.dumps({'event':'transcribed','device':device,'round':run,'seconds':end-start,'mel_ms':(mel_end-start)*1000,'encoder_ms':(enc_end-mel_end)*1000,'decoder_ms':(end-enc_end)*1000,'tokens':len(ids)-len(prompt),'eos':eos,'text':text}),flush=True)
