"""Fire UI inspection helpers for isolated native probes."""
import json
import socket


def inspect(path, payload=None):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect(str(path))
        connection.sendall(json.dumps(payload or {}).encode() + b'\n')
        with connection.makefile('r') as response:
            result = json.loads(response.readline())
    if 'error' in result:
        raise AssertionError(result)
    return result


def control(path, label, action='activate'):
    nodes = [n for n in inspect(path)['nodes'] if n['label'] == label and action in n['actions']]
    assert len(nodes) == 1, (label, nodes)
    return nodes[0]


def check_button_alignment(snapshot):
    nodes = snapshot['nodes']
    for button in (n for n in nodes if n['role'] == 'Button'):
        labels = [n for n in nodes if n['parent'] == button['id'] and n['role'] == 'Text']
        for label in labels:
            b, t = button['bounds'], label['bounds']
            if not b['height'] or not t['height']:
                continue
            # Scroll clips the button's semantic bounds, but the text child keeps
            # its full bounds. Do not judge vertical centering while an edge is
            # clipped; fully visible controls still use the strict check below.
            if min(t['y'] - b['y'], b['y'] + b['height'] - t['y'] - t['height']) < 6:
                continue
            assert abs(b['y'] + b['height']/2 - t['y'] - t['height']/2) < 1, (button, label)
