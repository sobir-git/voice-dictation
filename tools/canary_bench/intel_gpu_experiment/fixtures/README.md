The five-second float32/16 kHz mono fixture is synthetic speech from the
original isolated benchmark (`benchmark.py`, first passage, cropped to
320,000 bytes). It contains no user recording. The cut splits an utterance;
use it for latency and output stability, not reference WER.

Human fixtures are regenerated with `prepare_fixtures.py` from the pinned
public LibriSpeech dummy dataset. Its 73 recordings are all one speaker;
it does not establish general speech accuracy.
