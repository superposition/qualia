"""Bounded read-only mirror of robot status; retains the robot publication time."""
import argparse, json, os, time, urllib.request
from pathlib import Path

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--url', required=True)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--duration', type=int, default=1200)
    args = parser.parse_args()
    if not 1 <= args.duration <= 1800:
        parser.error('duration must be 1..1800 seconds')
    end = time.monotonic() + args.duration
    last_error = None
    while time.monotonic() < end:
        try:
            with urllib.request.urlopen(args.url, timeout=2) as response:
                raw = response.read(65537)
            if len(raw) > 65536: raise ValueError('status exceeds 64 KiB')
            value = json.loads(raw)
            if not isinstance(value, dict): raise ValueError('status must be an object')
            stamp = value.get('published_ms')
            if value.get('schema') == 'qualia.flyvis-live.v1':
                ns = value.get('published_unix_ns')
                stamp = ns // 1_000_000 if type(ns) is int else None
            if type(stamp) is not int or stamp <= 0 or stamp > int(time.time() * 1000) + 100: raise ValueError('invalid robot timestamp')
            temp = args.output.with_suffix('.tmp')
            temp.write_bytes(raw)
            os.replace(str(temp), str(args.output))
            if last_error: print('robot status recovered', flush=True)
            last_error = None
        except (OSError, ValueError) as error:
            if str(error) != last_error: print('robot status unavailable: ' + str(error), flush=True)
            last_error = str(error)
        time.sleep(0.25)

if __name__ == '__main__': main()
