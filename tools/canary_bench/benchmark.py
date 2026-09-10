#!/usr/bin/env python3
"""Generate synthetic fixtures and compare resident Canary backends sequentially."""
import argparse
import ast
import json
import hashlib
import platform
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
import time



def words(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def errors(reference, actual):
    before = list(range(len(actual) + 1))
    for i, word in enumerate(reference, 1):
        row = [i]
        for j, other in enumerate(actual, 1):
            row.append(min(row[-1] + 1, before[j] + 1, before[j-1] + (word != other)))
        before = row
    return before[-1]


def fixtures(root):
    source = ast.parse((Path(__file__).resolve().parents[1] / 'canary_long_probe.py').read_text())
    passages = next(ast.literal_eval(n.value) for n in ast.walk(source)
                    if isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'passages' for t in n.targets))
    texts = {
        'short': passages[0],
        'medium': ' '.join(passages[1:4]),
        'long': ' '.join(passages),
        'technical': 'Please open the settings panel and select a different speech recognition model. '
                     'Then retry the failed recording from history. The application should preserve the original audio, '
                     'keep the microphone off during playback, and show a clear error if the model cannot finish. '
                     'Save the final report in the project folder.',
        'silence': '',
    }
    for name, text in texts.items():
        path = root / (name + '.f32')
        if text:
            wav = root / (name + '.wav')
            # espeak has process-global callback state. Isolate each fixture.
            subprocess.run([sys.executable, '-c',
                'import sys; from migration_probe import synthesize; synthesize(sys.argv[1], sys.stdin.read())',
                str(wav)], input=text, text=True, check=True,
                env={**os.environ, 'PYTHONPATH': str(Path(__file__).resolve().parents[1])})
            subprocess.run(['ffmpeg', '-y', '-v', 'error', '-i', str(wav), '-ar', '16000',
                            '-ac', '1', '-f', 'f32le', str(path)], check=True)
        else:
            path.write_bytes(bytes(16000 * 4 * 3))
    (root / 'references.json').write_text(json.dumps(texts, indent=2))
    return texts


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--gguf', required=True)
    p.add_argument('--onnx', required=True)
    p.add_argument('--rounds', type=int, default=3)
    p.add_argument('--order', default='gguf,onnx')
    p.add_argument('--binary', type=Path, default=Path(__file__).parent/'target/release/canary-backend-bench')
    args = p.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    metadata = {
        'timestamp_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
        'platform': platform.platform(),
        'cpu': subprocess.check_output(['lscpu'], text=True),
        'thread_environment': {key: os.environ.get(key) for key in ('OMP_NUM_THREADS', 'OPENBLAS_NUM_THREADS', 'MKL_NUM_THREADS')},
        'rounds': args.rounds,
        'order': args.order,
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        'gguf_path': args.gguf,
        'onnx_path': args.onnx,
    }
    (root / 'metadata.json').write_text(json.dumps(metadata, indent=2))
    texts = fixtures(root)
    results = {}
    for backend in args.order.split(','):
        command = [str(args.binary.resolve()), backend, getattr(args, backend), str(args.rounds)]
        command.extend(str(root / (name + '.f32')) for name in texts)
        print('Running', backend, flush=True)
        with (root / (backend + '.stderr')).open('w') as log, (root / (backend + '.jsonl')).open('w') as out:
            proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=log, text=True)
            events = []
            for line in proc.stdout:
                out.write(line)
                out.flush()
                event = json.loads(line)
                events.append(event)
                print(backend, event['event'], event.get('round', ''),
                      Path(event.get('audio', '')).stem, round(event.get('seconds', 0), 3), flush=True)
            if proc.wait():
                raise RuntimeError(f'{backend} failed; see {log.name}')
        summary = {'load_seconds': events[0]['seconds'],
                   'peak_rss_mib': max(e.get('peak_rss_kib', 0) for e in events) / 1024,
                   'idle_cpu_seconds_over_2s': events[-1]['cpu_seconds_over_2s'], 'cases': {}}
        for name, reference in texts.items():
            case = [e for e in events if e.get('event') == 'transcribed' and Path(e['audio']).stem == name]
            runs = case[1:] if len(case) > 1 else case
            decoded = case[-1].get('text') or ''
            ref_words = words(reference)
            summary['cases'][name] = {'audio_seconds': case[0]['audio_seconds'],
                'first_seconds': case[0]['seconds'],
                'median_warm_seconds': statistics.median(e['seconds'] for e in runs),
                'word_error_rate': errors(ref_words, words(decoded)) / len(ref_words) if ref_words else None,
                'output_words': len(words(decoded)), 'errors': [e['error'] for e in case], 'text': decoded}
        results[backend] = summary
        (root / 'summary.json').write_text(json.dumps(results, indent=2))
        time.sleep(1)
    print(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
