#!/usr/bin/env python3
"""Build and measure opt-in Canary CPU experiments outside the app checkout."""
import argparse
import glob
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess

HERE = Path(__file__).resolve().parent
VARIANTS = {
    'baseline': {},
    'ln': {'CANARY_FUSE_LN': '1'},
    'q8x4': {'CANARY_Q8X4': '1'},
    'gemv': {'CANARY_GEMV_BIAS': '1'},
    'bucket': {'CANARY_BUCKET_STEP': '1'},
    'dw': {'CANARY_DIRECT_PRE_DW': '1'},
    'step1': {'CANARY_STEP_THREADS': '1'},
    'step2': {'CANARY_STEP_THREADS': '2'},
    'step4': {'CANARY_STEP_THREADS': '4'},
    'combined': {'CANARY_GEMV_BIAS': '1', 'CANARY_FUSE_LN': '1',
                 'CANARY_DIRECT_PRE_DW': '1'},
    'dw-step4': {'CANARY_DIRECT_PRE_DW': '1', 'CANARY_STEP_THREADS': '4'},
}


def environment(work, extra=None):
    env = dict(os.environ)
    for key in set().union(*(v.keys() for v in VARIANTS.values())) | {'CANARY_OP_PROFILE', 'BENCH_THREADS'}:
        env.pop(key, None)
    env.update(CARGO_TARGET_DIR=str(work / 'target'), CARGO_BUILD_JOBS='6')
    env.update(extra or {})
    return env


def prepare(args):
    work = args.work
    if (work / 'native').exists():
        raise SystemExit('Use a fresh work directory; existing native sources are not overwritten.')
    work.mkdir(parents=True, exist_ok=True)
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    sources = sorted(cargo_home.glob('registry/src/*/transcribe-cpp-sys-0.2.3'))
    if not sources:
        subprocess.run(['cargo', 'fetch', '--locked', '--manifest-path', str(HERE.parent / 'Cargo.toml')], check=True)
        sources = sorted(cargo_home.glob('registry/src/*/transcribe-cpp-sys-0.2.3'))
    if not sources:
        raise SystemExit('Could not locate transcribe-cpp-sys 0.2.3 after cargo fetch.')
    shutil.copytree(sources[0], work / 'native')
    for name in ['native.patch'] + (['profiling.patch'] if args.profile else []):
        subprocess.run(['git', 'apply', '--check', str(HERE / name)], cwd=work / 'native', check=True)
        subprocess.run(['git', 'apply', str(HERE / name)], cwd=work / 'native', check=True)
    for name in ('Cargo.toml', 'Cargo.lock', 'build.rs'):
        shutil.copy2(HERE.parent / name, work / name)
    with (work / 'Cargo.toml').open('a') as f:
        f.write('\n[patch.crates-io]\ntranscribe-cpp-sys = { path = "native" }\n')
    (work / 'src').mkdir(exist_ok=True)
    shutil.copy2(HERE / 'driver.rs', work / 'src/main.rs')
    subprocess.run(['cargo', 'build', '--release', '--manifest-path', str(work / 'Cargo.toml')],
                   env=environment(work), check=True)


def probe(args):
    work = args.work
    libraries = glob.glob(str(work / 'target/release/build/transcribe-cpp-sys-*/out/lib/libggml-cpu.a'))
    if len(libraries) != 1:
        raise SystemExit('Expected one native build; run prepare in a fresh work directory.')
    lib = Path(libraries[0]).parent
    include = work / 'native/ggml'
    cmd = ['c++', '-O3', '-march=native', '-std=c++17']
    for path in [include / 'include', include / 'src', include / 'src/ggml-cpu']:
        cmd += ['-I', str(path)]
    cmd += [str(HERE / 'kernel_probe.cpp'), str(lib / 'libggml-cpu.a'), str(lib / 'libggml-base.a'),
            '-fopenmp', '-lpthread', '-ldl', '-lm', '-o', str(work / 'kernel-probe')]
    subprocess.run(cmd, check=True)
    for name, flags in [('reference', {}), ('fused', {'CANARY_FUSE_LN': '1', 'CANARY_GEMV_BIAS': '1'})]:
        with (work / f'kernel-{name}.log').open('w') as f:
            subprocess.run([str(work / 'kernel-probe')], cwd=work,
                           env=environment(work, flags), stdout=f, check=True)
    for kind in ('ln', 'gemv'):
        if (work / f'{kind}-reference.bin').read_bytes() != (work / f'{kind}-fused.bin').read_bytes():
            raise SystemExit(f'{kind}: numerical mismatch')
    print('Q8 dots, layer norm, and matrix+bias checks passed with bit-identical outputs.')


def bench(args):
    if not args.gguf or not args.fixtures:
        raise SystemExit('bench requires --gguf and --fixtures.')
    variants = args.variants.split(',')
    if any(v not in VARIANTS for v in variants):
        raise SystemExit('Unknown variant. Available: ' + ','.join(VARIANTS))
    output = args.work / args.output
    output.mkdir(parents=True, exist_ok=False)
    cases = args.cases.split(',')
    audio = [str(args.fixtures / (case + '.f32')) for case in cases]
    metadata = {'variants': {v: VARIANTS[v] for v in variants}, 'rounds': args.rounds,
                'orders': args.orders, 'audio': audio, 'gguf': str(args.gguf),
                'profile': args.profile, 'timing': 'median of rounds after round zero'}
    (output / 'metadata.json').write_text(json.dumps(metadata, indent=2))
    summary = []
    for order in range(args.orders):
        for name in variants if order % 2 == 0 else reversed(variants):
            env = environment(args.work, VARIANTS[name])
            if args.profile:
                env['CANARY_OP_PROFILE'] = '1'
            p = subprocess.run([str(args.work / 'target/release/canary-backend-bench'),
                                'gguf', str(args.gguf), str(args.rounds), *audio],
                               capture_output=True, text=True, env=env, timeout=600)
            stem = f'{order}-{name}'
            (output / (stem + '.jsonl')).write_text(p.stdout)
            (output / (stem + '.stderr')).write_text(p.stderr)
            if p.returncode:
                raise SystemExit(f'{stem} failed; inspect its stderr.')
            rows = [r for r in map(json.loads, p.stdout.splitlines()) if r.get('event') == 'transcribed']
            if any(r['error'] for r in rows):
                raise SystemExit(f'{stem} returned an inference error.')
            for case in cases:
                samples = [r['seconds'] for r in rows if r['round'] > 0 and Path(r['audio']).stem == case]
                entry = {'order': order, 'variant': name, 'case': case,
                         'warm_seconds': samples, 'median_seconds': statistics.median(samples)}
                summary.append(entry)
                print(json.dumps(entry), flush=True)
    (output / 'summary.json').write_text(json.dumps(summary, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'probe', 'bench'])
    parser.add_argument('--work', type=Path, default=Path('/tmp/canary-kernel-reproduction'))
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--gguf', type=Path)
    parser.add_argument('--fixtures', type=Path)
    parser.add_argument('--variants', default='baseline,ln,q8x4,gemv,bucket,dw,combined')
    parser.add_argument('--cases', default='short,medium,long,technical,silence')
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--orders', type=int, default=2)
    parser.add_argument('--output', default='measurements')
    args = parser.parse_args()
    args.work = args.work.resolve()
    if args.rounds < 2 or args.orders < 1:
        parser.error('Use at least two rounds and one order.')
    globals()[args.command](args)
