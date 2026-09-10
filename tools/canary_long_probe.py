#!/usr/bin/env python3
"""Check long Canary dictations using generated speech and isolated user data."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import wave

from migration_probe import synthesize


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/release/speech-service')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    cache = os.environ.get('HF_HOME', str(Path.home() / '.cache/huggingface'))
    passages = [
        "The library opens early on Monday morning. Students can borrow books and read newspapers in the quiet room upstairs. Please return everything before the end of the month and keep your receipt.",
        "Our garden needs more water during the summer. The tomatoes are growing well beside the wooden fence. We should plant carrots next spring and ask the neighbors to help us remove the weeds.",
        "The airport was busy when we arrived yesterday. A friendly person showed us where to leave our bags. After a short wait we found our seats and watched the planes through the large windows.",
        "The kitchen renovation should finish next Friday. We ordered a new table and several comfortable chairs for the family. The workers will paint the walls after they install the lights above the sink.",
        "A telescope helps us see distant stars and planets. Tonight the sky should be clear enough for an excellent view. Bring a warm coat because the temperature usually falls after the sun goes down.",
        "The mountain trail begins near a small wooden bridge. Walk carefully across the stream and follow the signs toward the campsite. We should carry enough food and drinking water for everyone in our group.",
        "My keyboard stopped working during an important meeting. I borrowed another one from the office next door and finished writing the report. Tomorrow I will check the cable before buying a replacement from the shop.",
        "The museum has a wonderful collection of ancient pottery. Visitors can learn how people cooked their meals hundreds of years ago. A guide will explain the history of each room during the afternoon tour.",
        "The ocean looked calm when our boat left the harbor. We watched several birds flying above the water while the captain described our route. Everyone was excited to visit the island and explore its beaches.",
        "The dentist suggested making another appointment next month. I wrote the date in my calendar so I would remember it. The receptionist gave me a small card with the address and the office phone number.",
        "The orchestra rehearsed all afternoon before the concert. Several musicians stayed late to practice a difficult section together. When the doors finally opened the audience quickly filled the seats and waited for the conductor to arrive.",
        "The bakery sells fresh bread every morning. We bought a large loaf and some sweet rolls for breakfast. This is the final reminder for tomorrow. Please bring yellow umbrellas because the weather forecast predicts heavy rain.",
    ]
    speech = " ".join(passages)
    with tempfile.TemporaryDirectory(prefix='voice-canary-long-') as directory:
        root = Path(directory)
        audio = root / 'synthetic.wav'
        synthesize(audio, speech)
        # Reuse the existing model cache; Api::new uses HOME rather than HF_HOME.
        (root / '.cache').mkdir()
        (root / '.cache/huggingface').symlink_to(cache, target_is_directory=True)
        with wave.open(str(audio)) as recording:
            duration = recording.getnframes() / recording.getframerate()
        assert duration > 120, duration
        config = root / '.config/speech-to-text/config.yaml'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({
            'transcription': {'model': 'canary-180m-flash', 'language': 'en'},
            'audio': {'preprocess': False}, 'output': {'method': 'none'},
            'logging': {'file': str(root / 'service.log')},
        }))
        result = subprocess.run(
            [str(binary), '--transcribe', str(audio)],
            env={**os.environ, 'HOME': str(root), 'HF_HOME': cache},
            capture_output=True, text=True, timeout=240,
        )
        assert result.returncode == 0, result.stderr
        transcript = json.loads(result.stdout)['text'].lower()
        for marker in ('library', 'garden', 'airport', 'kitchen', 'telescope', 'mountain',
                       'keyboard', 'museum', 'ocean', 'dentist', 'orchestra', 'bakery', 'yellow umbrellas'):
            assert marker in transcript, f'Missing {marker!r}: {transcript}'
        print(json.dumps({'audio_seconds': round(duration, 2),
                          'transcript_words': len(transcript.split()),
                          'all_markers_present': True}))


if __name__ == '__main__':
    main()
