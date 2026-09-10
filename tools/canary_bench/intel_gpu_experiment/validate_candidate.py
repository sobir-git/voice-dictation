"""Sequential, bounded comparison on fixed clips; no app/config writes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, default=Path('/tmp/canary-vulkan-research'))
parser.add_argument('--model', type=Path, default=Path('/home/fire/.cache/huggingface/hub/models--handy-computer--canary-180m-flash-gguf/snapshots/b147f9dc52b59f0998e410540a84727bd86457fd/canary-180m-flash-Q8_0.gguf'))
parser.add_argument('--variants', default='baseline,cpu-tuned,hybrid8,hybrid8-f32')
parser.add_argument('--cycles', type=int, default=3)
parser.add_argument('--rounds', type=int, default=2)
parser.add_argument('--idle', action='store_true')
parser.add_argument('--fixture-set', choices=['fixed-five','complete-short'], default='fixed-five')
parser.add_argument('--deadline-epoch', type=float)
args = parser.parse_args()
if args.cycles < 1 or args.rounds < 2:
    parser.error('Use at least one cycle and two rounds (round zero is excluded from warm timing).')
r = args.root
out = r / ('final-validation' if args.fixture_set == 'fixed-five' else 'complete-short-validation')
out.mkdir(exist_ok=True)
if args.fixture_set == 'fixed-five':
    clips = [r/'five-second-screen.f32', *sorted((r/'validation-five').glob('*.f32'))]
    assert len(clips) == 9, 'Expected the original synthetic screen plus eight public speech clips.'
else:
    manifest = json.loads((r/'human/manifest.json').read_text())
    clips = [r/'human'/f"{x['case']}.f32" for x in manifest if x['seconds'] <= 5]
assert all(0 < p.stat().st_size <= 320000 and p.stat().st_size % 4 == 0 for p in clips)
common = {'OMP_WAIT_POLICY':'ACTIVE', 'BENCH_PAUSE_OMP':'1', 'CANARY_DIRECT_PRE_DW':'1'}
variants = {
    'baseline': ({'BENCH_BACKEND':'cpu','BENCH_THREADS':'8'}, None),
    'cpu-tuned': (dict(common, BENCH_BACKEND='cpu', BENCH_THREADS='4'), '0,2,4,6'),
    'hybrid8': (dict(common, BENCH_BACKEND='vulkan', BENCH_THREADS='8', CANARY_HYBRID='1'), '0-7'),
    'hybrid8-f32': (dict(common, BENCH_BACKEND='vulkan', BENCH_THREADS='8', CANARY_HYBRID='1', GGML_VK_DISABLE_F16='1'), '0-7'),
}
names = args.variants.split(',')
assert set(names) <= variants.keys() and 'baseline' in names
metadata = {'started_epoch':time.time(), 'model':str(args.model), 'model_sha256':hashlib.file_digest(args.model.open('rb'),'sha256').hexdigest(), 'variants':{n:variants[n] for n in names}, 'cycles':args.cycles, 'rounds':args.rounds, 'idle':args.idle, 'parent_cpu_affinity':sorted(os.sched_getaffinity(0)), 'clips':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in clips}}
(out/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
records = {name:[] for name in names}
for cycle in range(args.cycles):
    offset = cycle % len(names)
    for name in names[offset:]+names[:offset]:
        if args.deadline_epoch and time.time()+15 > args.deadline_epoch:
            raise SystemExit('Research deadline reached; refusing another process.')
        extra, affinity = variants[name]
        env = {k:v for k,v in os.environ.items() if not k.startswith(('CANARY_','GGML_VK_','BENCH_','OMP_','GOMP_'))}
        env.update(extra)
        if not args.idle:
            env['BENCH_SKIP_IDLE'] = '1'
        cmd = [str(r/'target/release/canary-backend-bench'),'gguf',str(args.model),str(args.rounds),*map(str,clips)]
        if affinity:
            cmd = ['taskset','-c',affinity,*cmd]
        p = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=15)
        (out/f'{cycle}-{name}.jsonl').write_text(p.stdout)
        (out/f'{cycle}-{name}.stderr').write_text(p.stderr)
        p.check_returncode()
        rows = list(map(json.loads,p.stdout.splitlines()))
        records[name].extend(rows)
        assert all(x.get('error') is None for x in rows), 'Inference error: inspect raw output.'
        total = sum(x['seconds'] for x in rows if x.get('event')=='transcribed' and x['round']>0)
        print(cycle, name, round(total,4), flush=True)
per_clip = []
for clip in clips:
    grouped = {n:[x for x in rows if x.get('event')=='transcribed' and x['audio']==str(clip) and x['round']>0] for n,rows in records.items()}
    medians = {n:statistics.median(x['seconds'] for x in rows) for n,rows in grouped.items()}
    texts = {n:sorted(set(x['text'] for x in rows)) for n,rows in grouped.items()}
    per_clip.append({'audio':clip.name,'medians':medians,'texts':texts,'texts_match':len(set(t for values in texts.values() for t in values))==1})
totals = {n:sum(x['medians'][n] for x in per_clip) for n in names}
summary = {'finished_epoch':time.time(), 'sum_of_clip_medians_seconds':totals, 'speedup_vs_baseline':{n:totals['baseline']/v for n,v in totals.items()}, 'all_texts_match':all(x['texts_match'] for x in per_clip), 'idle_cpu_seconds_over_2s':{n:[x['cpu_seconds_over_2s'] for x in rows if x.get('event')=='idle'] for n,rows in records.items()}, 'clips':per_clip}
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({k:v for k,v in summary.items() if k!='clips'},indent=2))
