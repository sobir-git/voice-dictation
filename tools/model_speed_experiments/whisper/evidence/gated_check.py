from pathlib import Path
import os,subprocess,json,statistics,re
r=Path('/tmp/whisper-research');manifest=json.loads((r/'public-extra/manifest.json').read_text());clips=[r/'public-extra'/f"{x['case']}.f32" for x in manifest];longer=json.loads((r/'longer-manifest.json').read_text());clips += [Path('/tmp/canary-vulkan-research/human')/f"{x['case']}.f32" for x in longer];clips += [Path('/tmp/voice-canary-benchmark/run2/silence.f32')]
model='/home/fire/.cache/huggingface/hub/models--Systran--faster-whisper-base.en/snapshots/3d3d5dee26484f91867d81cb899cfcf72b96be6c';variants={'baseline':({},None),'candidate':({'BENCH_THREADS':'8','BENCH_CUSTOM':'1','BENCH_FAST_MEL':'1','BENCH_PAD_SECONDS':'5','BENCH_MIN_SECONDS':'10','BENCH_SHORT_ONLY':'1'},'0-7')};records={n:[] for n in variants}
clips=clips[:5]+clips[-3:]
for cycle in range(1):
 for name in (['baseline','candidate'] if cycle==0 else ['candidate','baseline']):
  flags,pin=variants[name];env={k:v for k,v in os.environ.items() if not k.startswith(('BENCH_','OMP_','GOMP_','CT2_'))};env.update(flags)
  cmd=['/home/fire/projects/voice-dictation/target/release/whisper-speed-bench',model,'2',*map(str,clips)]
  if pin:cmd=['taskset','-c',pin,*cmd]
  p=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=45);(r/f'gated-check-{cycle}-{name}.jsonl').write_text(p.stdout);(r/f'gated-check-{cycle}-{name}.stderr').write_text(p.stderr);p.check_returncode();rows=list(map(json.loads,p.stdout.splitlines()));records[name]+=rows;print(cycle,name,sum(x['seconds'] for x in rows if x['event']=='transcribed' and x['round']==1),flush=True)
rows=[]
for clip in clips:
 group={n:[x for x in v if x['event']=='transcribed' and x['audio']==str(clip) and x['round']==1] for n,v in records.items()};med={n:statistics.median(x['seconds'] for x in v) for n,v in group.items()};texts={n:sorted(set(x['text'] for x in v)) for n,v in group.items()};norm=lambda t:tuple(re.findall(r"[a-z0-9]+(?:'[a-z0-9]+)?",t.lower()));rows.append({'audio':str(clip),'medians':med,'texts':texts,'words_match':len(set(norm(t) for v in texts.values() for t in v))==1})
short=rows[:5];totals={n:sum(x['medians'][n] for x in short) for n in variants};out={'short_totals':totals,'short_speedup':totals['baseline']/totals['candidate'],'all_words_match':all(x['words_match'] for x in rows),'cases':rows,'idle':{n:[x for x in v if x['event']=='idle'] for n,v in records.items()}};(r/'gated-check-summary.json').write_text(json.dumps(out,indent=2)+'\n');print(json.dumps({k:v for k,v in out.items() if k!='cases'}))
