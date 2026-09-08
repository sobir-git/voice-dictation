#!/usr/bin/env python3
"""Synthetic speech comparison; never records audio or writes live configuration."""
import ctypes
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import wave


def synthesize(path, text):
    library = ctypes.CDLL('libespeak-ng.so.1')
    samples = bytearray()
    callback_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.POINTER(ctypes.c_short), ctypes.c_int, ctypes.c_void_p)
    @callback_type
    def callback(data, count, events):
        if data and count:
            samples.extend(ctypes.string_at(data, count * 2))
        return 0
    rate = library.espeak_Initialize(1, 0, None, 0)
    library.espeak_SetSynthCallback(callback)
    library.espeak_SetVoiceByName(b'en-us')
    payload = text.encode()
    library.espeak_Synth(payload, len(payload) + 1, 0, 0, 0, 1, None, None)
    library.espeak_Synchronize()
    with wave.open(str(path), 'wb') as output:
        output.setparams((1, 2, rate, 0, 'NONE', 'not compressed'))
        output.writeframes(samples)


def main():
    project = Path(__file__).resolve().parents[1]
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default=str(project/'target/release/speech-service'))
    parser.add_argument('--baseline', action='store_true')
    args = parser.parse_args()
    from huggingface_hub import snapshot_download
    model = snapshot_download('Systran/faster-whisper-base.en', local_files_only=True)
    with tempfile.TemporaryDirectory(prefix='voice-rust-migration-') as directory:
        root = Path(directory)
        audio = root/'synthetic.wav'
        synthesize(audio, 'The meeting starts at nine tomorrow morning. Please send the updated document before lunch. We need a reliable voice dictation tool that keeps our words private.')
        configuration = root/'.config/speech-to-text/config.yaml'
        configuration.parent.mkdir(parents=True)
        results = {}
        for vad in (False, True):
            configuration.write_text(json.dumps({'transcription': {'model': model, 'beam_size': 5, 'vad_filter': vad}, 'audio': {'preprocess': False}, 'output': {'method': 'none'}}))
            result = json.loads(subprocess.check_output([args.binary, '--transcribe', str(audio)], env={**os.environ, 'HOME': str(root)}, timeout=180))
            assert 'meeting' in result['text'].lower(), result
            assert 'document' in result['text'].lower(), result
            results['rust_vad_'+str(vad).lower()] = result
        long_audio = root/'long-synthetic.wav'
        synthesize(long_audio, 'First, the meeting starts tomorrow morning and we should prepare the documents before lunch. Second, the development team will review the latest changes and fix the remaining problems. Third, please make sure the application keeps working when the main window is closed. Fourth, our recording indicator should appear near the mouse pointer without taking keyboard focus. Fifth, the history database must preserve every existing note during the migration. Finally, we can release the update when all checks have passed and the speech recognition works reliably.')
        result = json.loads(subprocess.check_output([args.binary, '--transcribe', str(long_audio)], env={**os.environ, 'HOME': str(root)}, timeout=180))
        assert all(word in result['text'].lower() for word in ('first', 'second', 'third', 'fourth', 'fifth', 'finally')), result
        results['rust_long_recording'] = result
        silence = root/'silence.wav'
        with wave.open(str(silence), 'wb') as output:
            output.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
            output.writeframes(bytes(32000))
        result = json.loads(subprocess.check_output([args.binary, '--transcribe', str(silence)], env={**os.environ, 'HOME': str(root)}, timeout=180))
        assert result['text'] == '', result
        results['rust_silence'] = result
        if args.baseline:
            from faster_whisper import WhisperModel
            baseline = WhisperModel(model, compute_type='int8', cpu_threads=4)
            started = time.monotonic()
            segments, _ = baseline.transcribe(str(audio), language='en', beam_size=5, vad_filter=True)
            text = ' '.join(segment.text.strip() for segment in segments)
            results['python'] = {'text': text, 'transcribe_seconds': time.monotonic()-started}
        print(json.dumps(results, indent=2))
        output = project/'artifacts/migration'
        output.mkdir(parents=True, exist_ok=True)
        (output/'speech-comparison.json').write_text(json.dumps(results, indent=2))

if __name__ == '__main__':
    main()
