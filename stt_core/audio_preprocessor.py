"""
Optional audio preprocessing using ffmpeg (if available).
Applies highpass filter and loudness normalization to improve transcription accuracy.
"""

import logging
import os
import shutil
import subprocess

logger = logging.getLogger(__name__)

FFMPEG_AVAILABLE: bool | None = None


def is_ffmpeg_available() -> bool:
    global FFMPEG_AVAILABLE
    if FFMPEG_AVAILABLE is None:
        FFMPEG_AVAILABLE = shutil.which('ffmpeg') is not None
        if FFMPEG_AVAILABLE:
            logger.info('ffmpeg found, audio preprocessing enabled')
        else:
            logger.info('ffmpeg not found, audio preprocessing disabled')
    return FFMPEG_AVAILABLE


def preprocess_audio(input_file: str, output_file: str | None = None) -> str:
    """
    Preprocess audio file with ffmpeg if available.
    Applies: highpass filter (80Hz), lowpass filter (8kHz), loudness normalization.
    
    Args:
        input_file: Path to input WAV file
        output_file: Path to output file (default: input_file with _processed suffix)
    
    Returns:
        Path to processed file (or original if ffmpeg unavailable)
    """
    if not is_ffmpeg_available():
        return input_file

    if not os.path.exists(input_file):
        return input_file

    if output_file is None:
        base, ext = os.path.splitext(input_file)
        output_file = f"{base}_processed{ext}"

    try:
        cmd = [
            'ffmpeg',
            '-y',
            '-i', input_file,
            '-af', 'highpass=f=80,lowpass=f=8000,loudnorm=I=-16:TP=-1.5:LRA=11',
            '-ar', '16000',
            '-ac', '1',
            '-sample_fmt', 's16',
            output_file,
        ]

        result = subprocess.run(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=30,
        )

        if result.returncode == 0 and os.path.exists(output_file):
            logger.debug('Audio preprocessed: %s -> %s', input_file, output_file)
            return output_file
        else:
            logger.warning('ffmpeg preprocessing failed: %s', result.stderr.decode()[:200])
            return input_file

    except subprocess.TimeoutExpired:
        logger.warning('ffmpeg preprocessing timed out')
        return input_file
    except Exception as e:
        logger.warning('ffmpeg preprocessing error: %s', e)
        return input_file


def cleanup_processed(processed_file: str, original_file: str) -> None:
    """Remove processed file if it differs from original."""
    if processed_file != original_file and os.path.exists(processed_file):
        try:
            os.unlink(processed_file)
        except Exception:
            pass
