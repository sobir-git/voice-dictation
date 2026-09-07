import glob
import logging
import selectors
import threading
import time

from evdev import InputDevice, ecodes

logger = logging.getLogger(__name__)


class HotkeyListener:
    def __init__(self, trigger_key, on_key_down, on_key_up, on_error, is_running, is_enabled):
        self.trigger_key = trigger_key
        self.on_key_down = on_key_down
        self.on_key_up = on_key_up
        self.on_error = on_error
        self.is_running = is_running
        self.is_enabled = is_enabled
        self._thread = None
        self._stop = threading.Event()
        self._pressed_sources = set()

    def validate(self):
        if self.trigger_key not in ecodes.ecodes or not self.trigger_key.startswith(('KEY_', 'BTN_')):
            raise ValueError(f'Unknown hotkey: {self.trigger_key}')

    def start(self, daemon=True):
        self.validate()
        if self._thread and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, name='hotkey-listener', daemon=daemon)
        self._thread.start()

    def stop(self):
        self._stop.set()
        if self._thread and self._thread is not threading.current_thread():
            self._thread.join(timeout=3)
            if self._thread.is_alive():
                raise RuntimeError('Hotkey listener did not stop')

    def _remove(self, selector, devices, path):
        dev = devices.pop(path)
        try:
            selector.unregister(dev)
        except (KeyError, ValueError):
            pass
        dev.close()
        was_pressed = path in self._pressed_sources
        self._pressed_sources.discard(path)
        if was_pressed and not self._pressed_sources:
            self.on_key_up()
        logger.debug('Input device removed: %s', path)

    def _open_devices(self, selector, devices, key_code):
        paths = set(glob.glob('/dev/input/event*'))
        for path in set(devices) - paths:
            self._remove(selector, devices, path)
        # Existing descriptors stay open; do not reopen every device every second.
        for path in paths - set(devices):
            dev = None
            try:
                dev = InputDevice(path)
                if key_code not in dev.capabilities().get(ecodes.EV_KEY, []):
                    dev.close()
                    continue
                # Ignore our own output keyboard to avoid feedback loops.
                if any(name in dev.name.lower() for name in ('ydotool', 'dotool')):
                    dev.close()
                    continue
                selector.register(dev, selectors.EVENT_READ)
                devices[path] = dev
                logger.info('Hotkey %s listening on %s (%s)', self.trigger_key, path, dev.name)
            except OSError:
                if dev is not None:
                    dev.close()

    def _event(self, device, event):
        source = device.path
        if event.type == ecodes.EV_SYN and event.code == ecodes.SYN_DROPPED:
            # A dropped release must not leave the microphone recording indefinitely.
            if source in self._pressed_sources:
                self._pressed_sources.discard(source)
                if not self._pressed_sources:
                    self.on_key_up()
            return
        if event.type != ecodes.EV_KEY or event.code != ecodes.ecodes[self.trigger_key]:
            return
        if event.value == 1 and self.is_enabled():
            inactive = not self._pressed_sources
            self._pressed_sources.add(source)
            if inactive:
                logger.debug('Dictation hotkey pressed on %s', source)
                self.on_key_down()
        elif event.value == 0:
            was_pressed = source in self._pressed_sources
            self._pressed_sources.discard(source)
            if was_pressed and not self._pressed_sources:
                logger.debug('Dictation hotkey released on %s', source)
                self.on_key_up()

    def _run(self):
        selector = selectors.DefaultSelector()
        devices = {}
        last_rescan = 0
        missing_reported = False
        try:
            while self.is_running() and not self._stop.is_set():
                now = time.monotonic()
                if now - last_rescan >= 2:
                    self._open_devices(selector, devices, ecodes.ecodes[self.trigger_key])
                    last_rescan = now
                    if not devices and not missing_reported:
                        self.on_error(RuntimeError(f'No readable keyboard supports {self.trigger_key}. Waiting for a keyboard.'))
                    missing_reported = not devices
                for key, _ in selector.select(timeout=0.2):
                    if self._stop.is_set():
                        break
                    try:
                        # Always drain events while paused; unread descriptors cause a busy loop.
                        for event in key.fileobj.read():
                            self._event(key.fileobj, event)
                    except BlockingIOError:
                        continue
                    except OSError as exc:
                        logger.warning('Input device disconnected: %s: %s', key.fileobj.path, exc)
                        self._remove(selector, devices, key.fileobj.path)
        except Exception as exc:
            logger.exception('Hotkey listener failed')
            if not self._stop.is_set():
                self.on_error(exc)
        finally:
            self._pressed_sources.clear()
            for device in devices.values():
                device.close()
            selector.close()
            logger.debug('Hotkey listener stopped')
