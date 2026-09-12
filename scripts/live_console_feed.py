"""One bounded live producer; status publication errors do not terminate it."""
from pathlib import Path
import argparse
import http.server
import threading
import json, os, socket, struct, subprocess, sys, time


def publish_status(run, state):
    """Publish each diagnostic snapshot independently, retrying on the next poll.

    Windows readers can temporarily deny destination replacement. Keeping the
    old atomic snapshot is preferable to terminating the live producer. Its
    unchanged publication timestamp lets consumers correctly mark it stale.
    """
    state['published_ms'] = int(time.time() * 1000)
    payload = json.dumps(state)
    errors = []
    for destination in (run / 'status.json', run.parent / 'live-status.json'):
        temporary = destination.with_name(destination.name + '.tmp')
        try:
            temporary.write_text(payload, encoding='utf8')
            os.replace(str(temporary), str(destination))
        except OSError as error:
            errors.append(str(error))
    return errors


def report_status_warning(warning):
    try:
        print('live-feed: status publication ' + (warning or 'recovered'), file=sys.stderr, flush=True)
    except OSError:
        # A failed optional log sink must not turn a status fault into a
        # fatal producer fault either. The saved snapshot still ages out.
        pass


def main():
    parser = argparse.ArgumentParser(description='Bounded, observation-only live CNS stream bridge')
    parser.add_argument('--camera', required=True)
    parser.add_argument('--artifact', required=True)
    parser.add_argument('--producer', required=True)
    parser.add_argument('--runtime', required=True)
    parser.add_argument('--listen', default='127.0.0.1')
    parser.add_argument('--status-port', type=int)
    parser.add_argument('--duration', type=int, default=1800)
    options = parser.parse_args()
    if not 1 <= options.duration <= 1800:
        parser.error('duration must be 1..1800 seconds')
    camera = options.camera
    run = Path(options.runtime) / time.strftime('live-%Y%m%d-%H%M%S')
    run.mkdir(parents=True)
    server = socket.socket()
    server.bind((options.listen, 18762))
    server.listen(4)
    status_server = None
    if options.status_port is not None:
        class StatusHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path != '/live-status.json':
                    self.send_error(404); return
                try:
                    payload = (run / 'status.json').read_bytes()
                except OSError:
                    self.send_error(503); return
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                try: self.wfile.write(payload)
                except OSError: pass
            def log_message(self, *args): pass
        status_server = http.server.ThreadingHTTPServer((options.listen, options.status_port), StatusHandler)
        status_server.daemon_threads = True
        threading.Thread(target=status_server.serve_forever, daemon=True).start()
    (run.parent / 'live-run.txt').write_text(str(run), encoding='utf8')
    server.setblocking(False)
    args = [options.producer, 'loop',
            '--artifact', options.artifact, '--camera', camera,
            '--ticks', '20000', '--device', 'gpu', '--spikes', str(run / 'spikes.bin'),
            '--trace', str(run / 'trace.csv'), '--sensors', camera.rsplit('/', 2)[0] + '/telemetry/compact',
            '--fusion-evidence', str(run / 'fusion.jsonl'), '--session', 'console-live-sensors', '--max-wheel-speed', '0']
    log = (run / 'producer.log').open('wb')
    producer = subprocess.Popen(args, stdout=log, stderr=log, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    deadline = time.monotonic() + options.duration
    clients = []
    reader = None
    buffer = b''
    header = None
    latest = None
    state = dict(camera=camera, producer_pid=producer.pid, bridge_pid=os.getpid(),
                 stream=f'tcp://{options.listen}:18762', producer_host=socket.gethostname(), state='starting', run_id=run.name, transport_attached=False, frames=0, tick=None,
                 firing=0, clients=0, max_duration_seconds=options.duration)
    last_status = 0
    fusion_reader = None
    fusion_buffer = b''
    status_warning = None
    def save_status():
        nonlocal status_warning
        errors = publish_status(run, state)
        warning = '; '.join(errors) if errors else None
        if warning != status_warning:
            report_status_warning(warning)
            status_warning = warning
    try:
        while time.monotonic() < deadline:
            if reader is None and (run / 'spikes.bin').exists():
                reader = (run / 'spikes.bin').open('rb')
            if reader:
                buffer += reader.read()
            if header is None and len(buffer) >= 8:
                header, buffer = buffer[:8], buffer[8:]
                if header != b'QLSP\x01\x00\x00\x00':
                    raise RuntimeError('Producer returned an invalid spike header')
            if header:
                try:
                    client, _ = server.accept()
                except BlockingIOError:
                    pass
                else:
                    try:
                        client.settimeout(0.25)
                        client.sendall(header)
                        clients.append(client)
                    except OSError:
                        client.close()
            while len(buffer) >= 20:
                tick, stamp, count = struct.unpack('<QQI', buffer[:20])
                if count > 166700:
                    raise RuntimeError('Producer firing count exceeds connectome size')
                length = 20 + 4 * count
                if len(buffer) < length:
                    break
                latest, buffer = buffer[:length], buffer[length:]
                state.update(state='live', frames=state['frames'] + 1, tick=tick,
                             firing=count, received_at=time.time())
                for client in clients[:]:
                    try:
                        client.sendall(latest)
                    except OSError:
                        client.close()
                        clients.remove(client)
            if fusion_reader is None and (run / 'fusion.jsonl').exists():
                fusion_reader = (run / 'fusion.jsonl').open('rb')
            if fusion_reader:
                fusion_buffer += fusion_reader.read()
                while b'\n' in fusion_buffer:
                    line, fusion_buffer = fusion_buffer.split(b'\n', 1)
                    if line:
                        entry = json.loads(line)
                        entry['run_id'] = run.name
                        key = 'output' if entry['schema_version'].endswith('.output.v1') else 'input'
                        state[key] = entry
            if state.get('received_at') and time.time() - state['received_at'] > 1.5:
                state['state'] = 'waiting_for_fresh_input'
            state['clients'] = len(clients)
            if time.monotonic() - last_status >= 0.25:
                save_status()
                last_status = time.monotonic()
            if producer.poll() is not None:
                state.update(state='finished' if producer.returncode == 0 else 'failed', exit_code=producer.returncode)
                break
            time.sleep(0.02)
        else:
            state['state'] = 'duration_limit_reached'
    finally:
        if producer.poll() is None:
            producer.terminate()
            producer.wait(timeout=10)
        for client in clients:
            client.close()
        server.close()
        if status_server:
            status_server.shutdown()
            status_server.server_close()
        if reader:
            reader.close()
        if fusion_reader:
            fusion_reader.close()
        log.close()
        save_status()


if __name__ == '__main__':
    main()
