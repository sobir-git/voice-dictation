"""Recreate public/synthetic fixtures only. Requires numpy, pyarrow, soundfile."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import urllib.request
import pyarrow.parquet as pq
import soundfile as sf

URL = 'https://huggingface.co/datasets/hf-internal-testing/librispeech_asr_dummy/resolve/main/clean/validation-00000-of-00001.parquet'
SHA = '4e69a06fa5edc90921e5e7e39a7084881f8b3ed9c805c574f4f39c6fde27c603'
parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, default=Path('/tmp/canary-vulkan-research'))
args = parser.parse_args()
r = args.root
r.mkdir(parents=True, exist_ok=True)
parquet = r/'human-speech.parquet'
if not parquet.exists():
    with urllib.request.urlopen(URL, timeout=30) as response:
        data = response.read(20_000_001)
    assert len(data) <= 20_000_000 and hashlib.sha256(data).hexdigest() == SHA
    parquet.write_bytes(data)
assert hashlib.file_digest(parquet.open('rb'),'sha256').hexdigest() == SHA
human, cropped = r/'human', r/'validation-five'
human.mkdir(exist_ok=True)
cropped.mkdir(exist_ok=True)
manifest = []
for row in pq.read_table(parquet).to_pylist():
    samples, rate = sf.read(io.BytesIO(row['audio']['bytes']), dtype='float32')
    assert rate == 16000 and samples.ndim == 1
    (human/f"{row['id']}.f32").write_bytes(samples.astype('<f4').tobytes())
    manifest.append({'case':row['id'],'seconds':len(samples)/rate,'reference':row['text'],'speaker_id':row['speaker_id']})
(human/'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
(human/'source.json').write_text(json.dumps({'url':URL,'sha256':SHA,'speaker_count':len(set(x['speaker_id'] for x in manifest))},indent=2)+'\n')
for p in sorted(human.glob('*.f32'))[:8]:
    (cropped/p.name).write_bytes(p.read_bytes()[:320000])
shutil.copyfile(Path(__file__).parent/'fixtures/five-second-screen.f32',r/'five-second-screen.f32')
print(f'Prepared {len(manifest)} public clips, eight cropped clips, and one synthetic screen in {r}')
