#!/usr/bin/env python3
"""Run under dbus-run-session; test the Rust tray with isolated service data."""
import asyncio
import json
import os
from pathlib import Path
import tempfile

from dbus_next import Variant
from dbus_next.aio import MessageBus
from dbus_next.service import ServiceInterface, method

WATCHER = 'org.kde.StatusNotifierWatcher'
ROOT = Path(__file__).resolve().parents[1]


class Watcher(ServiceInterface):
    def __init__(self):
        super().__init__(WATCHER)
        self.registered = asyncio.Event()
        self.service = None

    @method()
    def RegisterStatusNotifierItem(self, service: 's'):
        self.service = service
        self.registered.set()


async def main():
    host = await MessageBus().connect()
    watcher = Watcher()
    host.export('/StatusNotifierWatcher', watcher)
    await host.request_name(WATCHER)
    with tempfile.TemporaryDirectory(prefix='voice-rust-tray-') as directory:
        root = Path(directory)
        config = root/'.config/speech-to-text/config.yaml'
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({'output': {'method': 'none'}, 'notifications': {'enabled': False, 'audio_feedback': False}}))
        launcher = root/'run.sh'
        launcher.write_text('#!/bin/sh\nprintf opened > "$(dirname "$0")/opened"\n')
        launcher.chmod(0o700)
        binary = os.environ.get('STT_TEST_BINARY', str(ROOT/'target/debug/speech-service'))
        process = await asyncio.create_subprocess_exec(binary, '--probe', '--probe-tray', env={**os.environ, 'HOME': str(root), 'STT_SOCKET_PATH': str(root/'run/daemon.sock'), 'VOICE_DICTATION_PROJECT': str(root)}, stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL)
        try:
            await asyncio.wait_for(watcher.registered.wait(), 10)
            name = watcher.service
            xml = await host.introspect(name, '/StatusNotifierItem')
            item = host.get_proxy_object(name, '/StatusNotifierItem', xml).get_interface('org.kde.StatusNotifierItem')
            assert await item.get_id() == 'voice-dictation'
            await item.call_activate(0, 0)
            for _ in range(50):
                if (root/'opened').exists():
                    break
                await asyncio.sleep(.05)
            assert (root/'opened').exists(), 'Tray did not open the native launcher'
            menu_path = await item.get_menu()
            xml = await host.introspect(name, menu_path)
            menu = host.get_proxy_object(name, menu_path, xml).get_interface('com.canonical.dbusmenu')
            _, layout = await menu.call_get_layout(0, -1, [])
            toggle = next(child.value[0] for child in layout[2] if child.value[1].get('label', Variant('s', '')).value == 'Pause dictation')
            await menu.call_event(toggle, 'clicked', Variant('i', 0), 0)
            for _ in range(50):
                if 'Paused' in await item.get_title():
                    break
                await asyncio.sleep(.05)
            assert 'Paused' in await item.get_title()
            await host.release_name(WATCHER)
            watcher.registered.clear()
            await host.request_name(WATCHER)
            await asyncio.wait_for(watcher.registered.wait(), 5)
            _, layout = await menu.call_get_layout(0, -1, [])
            quit_item = next(child.value[0] for child in layout[2] if child.value[1].get('label', Variant('s', '')).value == 'Quit Voice Dictation')
            await menu.call_event(quit_item, 'clicked', Variant('i', 0), 0)
            assert await asyncio.wait_for(process.wait(), 5) == 0
            assert not (root/'run/daemon.sock').exists()
            assert (root/'run/daemon.stopped').exists()
            print('Rust tray registration, launcher, pause, host restart and clean quit passed')
        finally:
            if process.returncode is None:
                process.terminate()
                await asyncio.wait_for(process.wait(), 5)
    host.disconnect()


if __name__ == '__main__':
    asyncio.run(main())
