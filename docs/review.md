# Review and reliability revamp

Reviewed on 2026-09-07. Changes are implemented in this checkout.

## Why dictation stopped working

The default PipeWire source was `Loopback Analog Surround 5.1`. The last retained recording was 2.558 seconds of exact zeros. Whisper correctly rejected it as silence. A two-second capture explicitly targeting the built-in digital microphone had a peak of 1767 and RMS of 314 in signed 16-bit samples.

The application now selects the digital microphone by its PipeWire node name. The system-wide default source is unchanged. The previous user config was backed up alongside `~/.config/speech-to-text/config.yaml` before the change. The existing `base.en`, beam size 5, and Right Ctrl choices are preserved.

## Findings and fixes

| Severity | Finding | Implemented change |
| --- | --- | --- |
| High | No microphone selection or visible explanation for silent recordings | Per-app microphone selection, an eight-second test, actual recording levels, and explicit zero-signal/no-speech errors |
| High | Every recording overwrote the same file while previous transcription threads could still be reading it | Unique temporary files, one ordered transcription worker, at most four pending jobs, and cleanup after processing |
| High | Settings reload could leave the old hotkey thread alive | Per-listener stop event and join; paused state preserved; busy reload rejected |
| High | IPC client tried to acquire its own non-reentrant lock on send failure | Close outside the send lock; socket shutdown interrupts reads; reconnect and daemon recovery |
| High | A second daemon could unlink the active socket | Per-user socket directory and an advisory singleton lock |
| High | Output commands ignored failure or silently dropped text | Checked subprocess exits and timeouts; text persisted in History before delivery |
| Medium | Paused listeners did not drain readable descriptors | Drain events while paused; track releases and unplugged keyboards; remove unconditional event-loop sleeps |
| Medium | Concurrent preload and transcription could load the model twice | Synchronized model loading; cached models open without a network check; unrelated settings preserve the model |
| Medium | Settings writes could truncate YAML or accept invalid types | Validation and atomic replacement of config files |
| Medium | Tray and daemon lifetimes depended on one another | Separate systemd user services; tray reconnects after daemon failure |
| Medium | History disappeared when no tray was connected | Daemon owns history writes, using the existing database and schema |
| Medium | Zero-delay X11 typing could drop Unicode characters | Minimum 12 ms delay for non-ASCII X11 text; ASCII retains the configured delay |
| Medium | Diagnostics lacked recording identity, levels, and phase timing | Rotating logs with PID/thread, per-dictation identity, sample arrival time, signal levels, phase timings, exceptions, and a live viewer |

The dark GTK window contains Dictation, History, Settings, and Diagnostics. It uses native dark controls throughout. History supports full-text search and copy. The main window and tray expose cancellation, and the floating recording indicator remains optional.

## Validation

- 28 automated regression tests cover overlapping recordings, bounded queues, shutdown, cancellation, failed output, atomic config saves, silent input, lost key releases, listener restarts, duplicate daemons, malformed IPC, and the previous client deadlock.
- An isolated GTK smoke script checks navigation, dark theme, status transitions, error dismissal, and history search. Screenshots of all four pages were inspected.
- An actual digital-microphone capture verified a nonzero signal, live level updates, clean stop, and temporary-file permissions of `0600`.
- Real `xdotool` delivery into an isolated GTK entry verified punctuation, leading option-like text, and an accented character.
- The installed `base.en` model transcribed a generated speech sample correctly. Cached loading took approximately 0.86 seconds. That one sample took 1.83 seconds with beam 5 and 1.40 seconds with beam 1; this is a smoke measurement, not a general speed or accuracy benchmark.
- An intentional idle-daemon SIGKILL recovered to a loaded model and listening state in approximately 4.1 seconds under systemd.

A complete spoken dictation into the user's chosen application still needs a user trial. Automated tests cover the components and failure paths without injecting test text into the user's desktop.

## Remaining boundaries

- Cancellation prevents future delivery of canceled jobs. It does not interrupt an already-running native model inference or retract text already delivered.
- Microphone startup still opens an `arecord` stream per dictation. The UI now distinguishes opening the microphone from receiving actual samples. An always-open stream could reduce startup latency but would keep microphone access active between dictations.
- The optional floating indicator is compositor-dependent on Wayland. GNOME Wayland still requires a functioning `ydotoold`; this machine's current session is X11 and uses `xdotool`.
- The existing `ui.cursor_indicator` name is retained for config compatibility.

See [Handy comparison](handy-comparison.md) for the reference project and further ideas.
