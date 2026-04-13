#!/usr/bin/env python3

import argparse

from speech_to_text.core.app_logging import setup_logging
from speech_to_text.core.config import Config
from speech_to_text.core.transcriber import Transcriber


def main(argv=None):
    p = argparse.ArgumentParser(description='Transcribe an audio file')
    p.add_argument('path')
    args = p.parse_args(argv)

    cfg = Config()
    setup_logging(cfg)

    t = Transcriber(
        model_name=cfg.get('transcription', 'model'),
        compute_type=cfg.get('transcription', 'compute_type'),
        language=cfg.get('transcription', 'language'),
        beam_size=cfg.get('transcription', 'beam_size'),
        vad_filter=cfg.get('transcription', 'vad_filter'),
    )
    print(t.transcribe_file(args.path))


if __name__ == '__main__':
    raise SystemExit(main())
