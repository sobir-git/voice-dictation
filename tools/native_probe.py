#!/usr/bin/env python3
"""Isolated Fire UI interaction checks with synthetic dictations; no microphone."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from PIL import ImageGrab


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/release/voice-dictation')
    parser.add_argument('--output', default='artifacts/native')
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='voice-native-') as temporary:
        with open(Path(temporary)/'display', 'w+') as display_file:
            display = subprocess.Popen(['Xvfb', '-displayfd', str(display_file.fileno()), '-screen', '0', '1400x1000x24', '-nolisten', 'tcp'], pass_fds=(display_file.fileno(),), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            app = None
            try:
                for _ in range(100):
                    display_file.seek(0)
                    number = display_file.read().strip()
                    if number:
                        break
                    time.sleep(.05)
                else:
                    raise RuntimeError('Xvfb did not start')
                env = {**os.environ, 'DISPLAY': ':'+number, 'LIBGL_ALWAYS_SOFTWARE': '1', 'FIRE_UI_PROFILE': '1'}
                env.pop('WAYLAND_DISPLAY', None)
                def x(*args):
                    return subprocess.check_output(['xdotool', *map(str,args)], env=env, stderr=subprocess.DEVNULL, timeout=5).decode().strip()
                with open(output/'native.log','w') as log:
                    app = subprocess.Popen([str(binary), '--demo'], env=env, stdout=log, stderr=log)
                    for _ in range(100):
                        try:
                            window = x('search','--onlyvisible','--name','^Voice Dictation$').splitlines()[-1]
                            break
                        except subprocess.CalledProcessError:
                            if app.poll() is not None:
                                raise RuntimeError('App exited; see native.log')
                            time.sleep(.05)
                    else:
                        raise RuntimeError('Window did not appear')
                    x('windowmove',window,0,0)
                    x('windowfocus',window)
                    time.sleep(.5)
                    def click(px,py):
                        x('mousemove','--window',window,px,py,'click',1)
                        time.sleep(.25)
                    def key(*keys):
                        x('key','--clearmodifiers',*keys)
                        time.sleep(.2)
                    def shot(name):
                        size=dict(line.split('=') for line in x('getwindowgeometry','--shell',window).splitlines() if '=' in line)
                        ImageGrab.grab(bbox=(0,0,int(size['WIDTH']),int(size['HEIGHT'])),xdisplay=env['DISPLAY']).save(output/(name+'.png'))
                        print('Screenshot:',name,flush=True)
                    shot('01-dictation')
                    click(297,329)
                    shot('02-paused')
                    click(84,242)
                    time.sleep(.4)
                    click(460,189)
                    shot('03-history')
                    click(455,135)
                    x('type','--clearmodifiers','--delay',5,'tomorrow')
                    time.sleep(.5)
                    click(450,191)
                    click(898,535)
                    copied=subprocess.check_output(['xclip','-selection','clipboard','-o'],env=env,timeout=3).decode()
                    assert 'Three ideas for tomorrow' in copied, copied
                    shot('03a-history-search')
                    click(82,286)
                    shot('04-settings')
                    click(620,177)
                    shot('05-model-menu')
                    key('Escape')
                    click(620,362)
                    click(952,794)
                    time.sleep(.4)
                    assert '"cmd":"save_config"' in (output/'native.log').read_text()
                    shot('05a-settings-applied')
                    click(82,334)
                    time.sleep(.4)
                    shot('06-diagnostics')
                    click(82,195)
                    time.sleep(.3)
                    ticks=lambda:sum(map(int,Path(f'/proc/{app.pid}/stat').read_text().split()[13:15]))
                    before=ticks()
                    time.sleep(2)
                    idle=ticks()-before
                    assert idle<=2, f'Idle CPU did not settle: {idle} ticks'
                    x('windowsize',window,680,760)
                    time.sleep(.4)
                    click(65,286)
                    shot('07-narrow-settings')
                    x('windowsize',window,420,360)
                    time.sleep(.4)
                    shot('08-minimum-settings')
                    x('mousemove','--window',window,330,163,'click','--repeat',8,'--delay',40,5)
                    time.sleep(.3)
                    shot('09-scrolled-settings')
                    # A synthetic passive HUD must preserve the dictation target's focus.
                    x('windowsize', window, 680, 760)
                    x('windowfocus', window)
                    hud_env = {**env, 'VOICE_DICTATION_HUD_POSITION': '260,180'}
                    hud = subprocess.Popen([str(binary), '--hud'], env=hud_env, stdin=subprocess.PIPE, stdout=log, stderr=log)
                    try:
                        hud.stdin.write(b'{"recording":true,"level":0.7}\n')
                        hud.stdin.flush()
                        for _ in range(100):
                            try:
                                hud_window = x('search', '--onlyvisible', '--name', '^Voice Dictation Status$').splitlines()[-1]
                                break
                            except subprocess.CalledProcessError:
                                if hud.poll() is not None:
                                    raise RuntimeError('HUD exited; see native.log')
                                time.sleep(.05)
                        else:
                            raise RuntimeError('HUD did not appear')
                        time.sleep(.3)
                        assert x('getwindowfocus') == window, 'HUD stole keyboard focus'
                        geometry = dict(line.split('=') for line in x('getwindowgeometry', '--shell', hud_window).splitlines() if '=' in line)
                        assert (geometry['WIDTH'], geometry['HEIGHT']) == ('280', '58'), geometry
                        import ctypes
                        xlib = ctypes.CDLL('libX11.so.6')
                        shape = ctypes.CDLL('libXext.so.6')
                        xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
                        xlib.XOpenDisplay.restype = ctypes.c_void_p
                        connection = xlib.XOpenDisplay(env['DISPLAY'].encode())
                        shape.XShapeGetRectangles.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
                        shape.XShapeGetRectangles.restype = ctypes.c_void_p
                        count, ordering = ctypes.c_int(), ctypes.c_int()
                        rectangles = shape.XShapeGetRectangles(connection, int(hud_window), 2, ctypes.byref(count), ctypes.byref(ordering))
                        assert count.value == 0, 'HUD intercepts mouse clicks'
                        xlib.XFree.argtypes = [ctypes.c_void_p]
                        xlib.XFree(rectangles)
                        xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]
                        xlib.XCloseDisplay(connection)
                        for level in [.2, .5, .9, .7, .4, .8, .3, .6] * 2:
                            hud.stdin.write((json.dumps({'level': level}) + '\n').encode())
                            hud.stdin.flush()
                            time.sleep(.025)
                        shot('10-floating-recording')
                        hud.stdin.write(b'{"recording":false}\n')
                        hud.stdin.flush()
                        time.sleep(.2)
                        shot('11-floating-transcribing')
                        hud.stdin.close()
                        hud.wait(timeout=5)
                        assert hud.returncode == 0
                        assert x('getwindowfocus') == window
                    finally:
                        if hud.poll() is None:
                            hud.terminate()
                            hud.wait(timeout=5)
                    key('Alt+F4')
                    # Xvfb has no window manager, so send WM_DELETE_WINDOW directly.
                    import ctypes
                    lib=ctypes.CDLL('libX11.so.6');lib.XOpenDisplay.argtypes=[ctypes.c_char_p];lib.XOpenDisplay.restype=ctypes.c_void_p
                    lib.XInternAtom.argtypes=[ctypes.c_void_p,ctypes.c_char_p,ctypes.c_int];lib.XInternAtom.restype=ctypes.c_ulong
                    class Event(ctypes.Structure):
                        _fields_=[('type',ctypes.c_int),('serial',ctypes.c_ulong),('send_event',ctypes.c_int),('display',ctypes.c_void_p),('window',ctypes.c_ulong),('message_type',ctypes.c_ulong),('format',ctypes.c_int),('data',ctypes.c_long*5),('pad',ctypes.c_long*5)]
                    d=lib.XOpenDisplay(env['DISPLAY'].encode());e=Event();e.type=33;e.display=d;e.window=int(window);e.message_type=lib.XInternAtom(d,b'WM_PROTOCOLS',0);e.format=32;e.data[0]=lib.XInternAtom(d,b'WM_DELETE_WINDOW',0)
                    lib.XSendEvent.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.c_int,ctypes.c_long,ctypes.c_void_p];lib.XSendEvent(d,int(window),0,0,ctypes.byref(e))
                    lib.XFlush.argtypes=[ctypes.c_void_p];lib.XFlush(d)
                    app.wait(timeout=5)
                    assert app.returncode==0
                    recorded=(output/'native.log').read_text()
                    assert '"cmd":"toggle_listening"' in recorded
                    assert '"cmd":"history"' in recorded
                    assert '"cmd":"diagnostics"' in recorded
                    metrics={'idle_cpu_ticks_over_2s':idle,'binary':str(binary),'native_interactions':'completed'}
                    (output/'metrics.json').write_text(json.dumps(metrics,indent=2))
                    print(json.dumps(metrics),flush=True)
            finally:
                if app and app.poll() is None:
                    app.terminate()
                    app.wait(timeout=5)
                display.terminate()
                display.wait(timeout=5)

if __name__=='__main__':
    main()
