import json,urllib.request,hashlib,io,concurrent.futures
from pathlib import Path
import soundfile as sf
r=Path('/tmp/whisper-research/public-extra')
def fetch(offset):
 url=f'https://datasets-server.huggingface.co/rows?dataset=openslr/librispeech_asr&config=clean&split=test&offset={offset}&length=35'
 with urllib.request.urlopen(url,timeout=20) as f:data=json.load(f)
 found=[]
 for x in data['rows']:
  row=x['row']
  if not 5<=len(row['text'].split())<=14:continue
  try:
   with urllib.request.urlopen(row['audio'][0]['src'],timeout=20) as f:audio=f.read(2_000_001)
   if len(audio)>2_000_000:continue
   samples,rate=sf.read(io.BytesIO(audio),dtype='float32')
   if rate!=16000 or samples.ndim!=1 or not 1<=len(samples)/rate<=5:continue
   raw=samples.astype('<f4').tobytes();(r/f"{row['id']}.f32").write_bytes(raw)
   found.append({'case':row['id'],'seconds':len(samples)/rate,'reference':row['text'],'speaker_id':row['speaker_id'],'row_index':x['row_idx'],'dataset':'openslr/librispeech_asr','config':'clean','split':'test','sha256_f32':hashlib.sha256(raw).hexdigest()})
   if len(found)==2:break
  except Exception as e:print('skip',offset,type(e).__name__,flush=True)
 return found
out=[]
with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
 for rows in pool.map(fetch,range(0,2400,200)):
  out.extend(rows);print('clips',len(out),flush=True)
out=list({x['case']:x for x in out}.values());(r/'manifest.json').write_text(json.dumps(out,indent=2)+'\n');print('done',len(out),'speakers',len(set(x['speaker_id'] for x in out)))
