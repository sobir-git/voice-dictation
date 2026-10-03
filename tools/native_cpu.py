"""Explicit native CPU build presets and pre-execution Linux host checks."""
from __future__ import annotations

import os
from pathlib import Path
import platform
import re

PRESETS = ('default', 'avx2')
ENV = 'VOICE_DICTATION_CPU_ISA'
AVX2_FLOOR = ('sse4_2', 'avx', 'avx2', 'fma', 'f16c', 'bmi2')
AVX2_CMAKE_ARGS = (
    '-DTRANSCRIBE_X86_CONSERVATIVE=OFF -DGGML_NATIVE=OFF -DGGML_SSE42=ON '
    '-DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_BMI2=ON '
    '-DGGML_AVX512=OFF -DGGML_AVX512_VBMI=OFF -DGGML_AVX512_VNNI=OFF '
    '-DGGML_AVX512_BF16=OFF -DGGML_AVX_VNNI=OFF'
)
DEFINES = dict(token[2:].split('=', 1) for token in AVX2_CMAKE_ARGS.split())


def preset(value):
    if value not in PRESETS:
        raise ValueError(f'{ENV}/--cpu-isa must be default or avx2')
    return value


def build_flags(environment):
    isa = preset(environment.get(ENV, 'default'))
    return {'cpu_isa': isa, 'cpu_isa_floor': list(AVX2_FLOOR) if isa == 'avx2' else []}


def provenance_isa(provenance):
    flags = provenance.get('build_flags', {})
    isa = preset(flags.get('cpu_isa', 'default'))
    expected = list(AVX2_FLOOR) if isa == 'avx2' else []
    if flags.get('cpu_isa_floor', []) != expected:
        raise ValueError('CPU ISA floor disagrees with the build preset')
    return isa


def selection(receipt, environment):
    if ENV in environment:
        return preset(environment[ENV]), 'explicit'
    if receipt is not None:
        return provenance_isa(receipt['provenance']), 'preserved_receipt'
    return 'default', 'default'


def require_host(isa):
    if preset(isa) == 'default':
        return
    if platform.system() != 'Linux' or platform.machine().lower() not in ('x86_64', 'amd64'):
        raise ValueError('AVX2 preset requires a Linux x86-64 host with OS-enabled AVX')
    try:
        # Linux exposes AVX only with usable OS XSAVE state. Check every exposed
        # processor so scheduling onto another core cannot violate the floor.
        rows = [set(line.split(':', 1)[1].split()) for line in Path('/proc/cpuinfo').read_text().splitlines()
                if line.split(':', 1)[0].strip() == 'flags']
    except OSError as error:
        raise ValueError('Cannot verify AVX2 CPU/OS compatibility') from error
    available = set.intersection(*rows) if rows else set()
    missing = sorted(set(AVX2_FLOOR) - available)
    if missing:
        raise ValueError('Incompatible AVX2 CPU/OS: missing ' + ', '.join(missing) +
                         '; use a default build instead (no candidate binary was executed)')


def _check_target_flags(name, value):
    if re.search(r'-march(?:=|\s)|-mcpu(?:=|\s)|/arch:|target-(?:cpu|feature)|'
                 r'-mavx512\w*|-mavxvnni\w*|-mavx10\w*|-mamx\w*|-mfma4\b|'
                 r'-mno-(?:avx\w*|fma|f16c|bmi2|sse4\.2)', value, re.I):
        raise ValueError(f'{name} conflicts with the fixed AVX2 floor')


def _cmake_args(value):
    _check_target_flags('CMake target flags', value)
    pattern = r'-D([A-Za-z0-9_]+)(?::[A-Za-z]+)?=("[^"]*"|[^\s]+)'
    for match in re.finditer(pattern, value):
        key, setting = match.group(1), match.group(2).strip('"').upper()
        expected = DEFINES.get(key)
        if key.startswith(('GGML_AVX512', 'GGML_AVX_VNNI')) or key == 'GGML_CPU_ALL_VARIANTS':
            expected = 'OFF'
        if expected is not None and setting != expected:
            raise ValueError(f'{key}={setting} conflicts with the fixed AVX2 preset ({expected})')
    if re.search(r'(?:^|\s)-(?:D\s+|U)', value):
        raise ValueError('AVX2 preset does not accept split -D or -U CMake overrides')
    # Keep unrelated options (including their double quoting) untouched. Native
    # build.rs applies both argument variables, not merely a fallback variable.
    return re.sub(pattern, lambda m: '' if m.group(1) in DEFINES else m.group(0), value).strip()


def compose(environment=None, isa=None):
    result = dict(os.environ if environment is None else environment)
    chosen = preset(isa if isa is not None else result.get(ENV, 'default'))
    if isa is not None:
        result[ENV] = chosen
    if chosen == 'default':
        return result
    if result.get('TRANSCRIBE_DIR'):
        raise ValueError('AVX2 preset cannot verify a prebuilt TRANSCRIBE_DIR; use pinned source builds')
    target = result.get('CARGO_BUILD_TARGET', '')
    if target and not target.startswith('x86_64-'):
        raise ValueError('AVX2 preset requires an x86_64 build target')
    for name, value in result.items():
        if name in ('CC', 'CXX', 'CFLAGS', 'CXXFLAGS', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS') or name.startswith((
                'CFLAGS_', 'CXXFLAGS_', 'CC_', 'CXX_', 'CARGO_TARGET_')):
            _check_target_flags(name, value)
    _cmake_args(result.get('CMAKE_ARGS', ''))
    other = _cmake_args(result.get('TRANSCRIBE_CMAKE_ARGS', ''))
    result['TRANSCRIBE_CMAKE_ARGS'] = AVX2_CMAKE_ARGS + (' ' + other if other else '')
    return result
