#!/usr/bin/env python3
"""Headless checks: direct HTTP, HTML5 website extraction, and HLS playback."""
import functools
import http.server
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time

class Handler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        # A real HTML page, not a media URL disguised by the test.
        if self.path == '/watch':
            page = b'<!doctype html><html><head><title>nvplay stream fixture</title></head><body><video controls src="/fixture.mp4"></video></body></html>'
            self.send_response(200)
            self.send_header('Content-Type', 'text/html')
            self.send_header('Content-Length', str(len(page)))
            self.end_headers()
            self.wfile.write(page)
            return
        if self.path == '/unsupported':
            page = b'<!doctype html><html><body>No video on this page.</body></html>'
            self.send_response(200)
            self.send_header('Content-Type', 'text/html')
            self.send_header('Content-Length', str(len(page)))
            self.end_headers()
            self.wfile.write(page)
            return
        super().do_GET()


def main():
    with tempfile.TemporaryDirectory(prefix='nvplay-stream-check-') as directory:
        root = Path(directory)
        ffmpeg = shutil.which('ffmpeg')
        if not ffmpeg:
            raise SystemExit('ffmpeg is required')
        subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error',
            '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=30',
            '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
            '-t', '15', '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '30',
            '-c:a', 'aac', '-movflags', '+faststart', str(root / 'fixture.mp4')], check=True)
        subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-i', str(root / 'fixture.mp4'),
            '-c', 'copy', '-hls_time', '1', '-hls_list_size', '0', str(root / 'stream.m3u8')], check=True)
        subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-i', str(root / 'fixture.mp4'),
            '-vf', 'scale=320:180', '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '30', '-c:a', 'copy',
            '-hls_time', '1', '-hls_list_size', '0', str(root / 'low.m3u8')], check=True)
        (root / 'master.m3u8').write_text('#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=320x180\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=9000000,RESOLUTION=640x360\nstream.m3u8\n')
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Handler, directory=directory))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for path in ['/fixture.mp4', '/watch', '/stream.m3u8']:
                env = os.environ.copy()
                env['NVPLAY_TEST_URL'] = f'http://127.0.0.1:{server.server_port}{path}'
                started = time.monotonic()
                subprocess.run(['cargo', 'test', '--locked', 'network_engine_loads_advances_and_stops', '--', '--ignored', '--nocapture'], env=env, check=True)
                print(f'{path}: passed in {time.monotonic() - started:.2f}s', flush=True)
            for quality, height in [('720', '180'), ('1080', '180'), ('original', '360')]:
                env = os.environ.copy()
                env['NVPLAY_TEST_URL'] = f'http://127.0.0.1:{server.server_port}/master.m3u8'
                env['NVPLAY_TEST_QUALITY'] = quality
                env['NVPLAY_TEST_HEIGHT'] = height
                subprocess.run(['cargo', 'test', '--locked', 'network_engine_loads_advances_and_stops', '--', '--ignored', '--nocapture'], env=env, check=True)
                print(f'HLS {quality}: selected expected {height}p fixture variant', flush=True)
            env = os.environ.copy()
            env['NVPLAY_TEST_URL'] = f'http://127.0.0.1:{server.server_port}/unsupported'
            subprocess.run(['cargo', 'test', '--locked', 'network_unsupported_page', '--', '--ignored'], env=env, check=True)
        finally:
            server.shutdown()
            server.server_close()

if __name__ == '__main__':
    main()
