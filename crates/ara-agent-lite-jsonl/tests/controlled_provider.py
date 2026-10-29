"""Owned loopback catalogue/SSE server for ordinary-process IPC checks."""
from contextlib import AbstractContextManager
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import time


def text(value, **extra):
    return {'delta': {'content': value}, 'finish': 'stop', **extra}


def call(operation_id, name, arguments):
    return {'delta': {'tool_calls': [{'index': 0, 'id': operation_id, 'type': 'function',
            'function': {'name': name, 'arguments': json.dumps(arguments)}}]}, 'finish': 'tool_calls'}


class ControlledProvider(AbstractContextManager):
    def __init__(self, replies, model='jsonl-fixture'):
        self.replies = list(replies)
        self.model = model
        self.requests = []
        self.catalog_requests = 0
        self.lock = threading.Lock()
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                assert self.path == '/v1/models'
                with outer.lock:
                    outer.catalog_requests += 1
                payload = json.dumps({'data': [{'id': outer.model, 'protocols': ['openai-completions'],
                    'context_window': 1000000, 'max_tokens': 4096,
                    'omp': {'supportsTools': True, 'input': ['text'], 'reasoning': False}}]}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_POST(self):
                length = int(self.headers['Content-Length'])
                assert self.path == '/v1/chat/completions' and 0 < length <= 4 * 1024 * 1024
                body = json.loads(self.rfile.read(length))
                with outer.lock:
                    number = len(outer.requests)
                    outer.requests.append(body)
                    response = outer.replies[number] if number < len(outer.replies) else text('No further scripted reply.')
                if callable(response):
                    response = response(body)
                time.sleep(response.get('delay_ms', 0) / 1000)
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                packets = response.get('deltas', [response['delta']])
                packets = [(dict(role='assistant'), None)] + [(p, None) for p in packets] + [({}, response['finish'])]
                try:
                    for delta, finish in packets:
                        packet = {'id': 'controlled-only', 'object': 'chat.completion.chunk',
                                  'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}
                        self.wfile.write(('data: ' + json.dumps(packet) + '\n\n').encode())
                    self.wfile.write(b'data: [DONE]\n\n')
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.endpoint = f'http://127.0.0.1:{self.server.server_port}/v1'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        assert not self.thread.is_alive()
