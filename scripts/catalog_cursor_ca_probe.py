"""Observe fixed Cursor H2 CA behavior on real clear/TLS loopback servers."""
import argparse, datetime, hashlib, json, os, pathlib, shutil, subprocess, time, uuid

BUN_SHA = '627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-run', required=True, type=pathlib.Path)
    parser.add_argument('--bun', required=True, type=pathlib.Path)
    parser.add_argument('--openssl', required=True, type=pathlib.Path)
    parser.add_argument('--output', required=True, type=pathlib.Path)
    args = parser.parse_args()
    assert hashlib.sha256(args.bun.read_bytes()).hexdigest() == BUN_SHA
    run = args.output / ('run-' + datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + uuid.uuid4().hex[:8])
    run.mkdir(parents=True)
    script = run / 'probe.mjs'
    shutil.copyfile(pathlib.Path(__file__).with_suffix('.mjs'), script)
    ca, ca_key, leaf, key, csr = [run / name for name in ['ca.pem', 'ca.key', 'leaf.pem', 'leaf.key', 'leaf.csr']]
    extensions = run / 'leaf.cnf'
    extensions.write_text('subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')

    def openssl(*options):
        result = subprocess.run([str(args.openssl), *map(str, options)], capture_output=True, timeout=15)
        result.check_returncode()

    openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2', '-subj', '/CN=ARA test root', '-addext', 'basicConstraints=critical,CA:TRUE', '-keyout', ca_key, '-out', ca)
    openssl('req', '-new', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=localhost', '-keyout', key, '-out', csr)
    openssl('x509', '-req', '-days', '2', '-in', csr, '-CA', ca, '-CAkey', ca_key, '-CAcreateserial', '-out', leaf, '-extfile', extensions)
    ready, receipts = run / 'ready.json', run / 'server-receipts.json'
    config = run / 'server.json'
    config.write_text(json.dumps({'cert': str(leaf.resolve()), 'key': str(key.resolve()), 'ready': str(ready.resolve()), 'receipts': str(receipts.resolve())}))
    clean_env = os.environ.copy()
    clean_env.pop('NODE_EXTRA_CA_CERTS', None)
    stderr = (run / 'server-stderr.log').open('wb')
    server = subprocess.Popen([str(args.bun), str(script.resolve()), '--server', str(args.source_run.resolve()), str(config.resolve())], env=clean_env, stdout=subprocess.DEVNULL, stderr=stderr)
    try:
        start = time.monotonic()
        while not ready.exists():
            if server.poll() is not None:
                raise RuntimeError('H2 server stopped: ' + (run / 'server-stderr.log').read_text())
            if time.monotonic() - start > 5:
                raise RuntimeError('H2 server readiness timeout')
            time.sleep(.01)
        ports = json.loads(ready.read_text())
        clear = f'http://127.0.0.1:{ports["clearPort"]}'
        tls = f'https://127.0.0.1:{ports["tlsPort"]}'
        valid = str(ca.resolve())
        missing = valid + '-missing'
        clients, client_configs = [], []
        for name, startup, steps in [
            ('unset-startup', None, [
                {'id': 'clear-unset', 'url': clear}, {'id': 'tls-unset', 'url': tls},
                {'id': 'clear-runtime-missing', 'url': clear, 'env': missing}, {'id': 'tls-runtime-missing', 'url': tls},
                {'id': 'clear-runtime-valid', 'url': clear, 'env': valid}, {'id': 'tls-runtime-valid', 'url': tls},
                {'id': 'tls-runtime-inline', 'url': tls, 'env': ca.read_text()},
            ]),
            ('valid-startup', valid, [
                {'id': 'clear-valid', 'url': clear}, {'id': 'tls-valid', 'url': tls},
                {'id': 'tls-after-missing', 'url': tls, 'env': missing}, {'id': 'tls-after-unset', 'url': tls, 'env': None},
            ]),
            ('missing-startup', missing, [
                {'id': 'clear-missing', 'url': clear}, {'id': 'tls-missing', 'url': tls},
                {'id': 'tls-after-valid', 'url': tls, 'env': valid},
            ]),
            ('inline-startup', ca.read_text(), [
                {'id': 'clear-inline', 'url': clear}, {'id': 'tls-inline', 'url': tls},
            ]),
            ('whitespace-startup', ' ' + valid + ' ', [
                {'id': 'clear-whitespace', 'url': clear}, {'id': 'tls-whitespace', 'url': tls},
            ]),
        ]:
            env = clean_env.copy()
            if startup is not None:
                env['NODE_EXTRA_CA_CERTS'] = startup
            client_file = run / (name + '.json')
            client_file.write_text(json.dumps({'steps': steps}))
            client_configs.append({'name': name, 'sha256': hashlib.sha256(client_file.read_bytes()).hexdigest()})
            result = subprocess.run([str(args.bun), str(script.resolve()), '--client', str(args.source_run.resolve()), str(client_file.resolve())], env=env, capture_output=True, timeout=15)
            (run / (name + '-stdout.log')).write_bytes(result.stdout)
            (run / (name + '-stderr.log')).write_bytes(result.stderr)
            result.check_returncode()
            clients.append({'name': name, **json.loads(result.stdout)})
        manifest = json.loads((args.source_run / 'source-manifest.json').read_text())
        cursor_source = next(entry for entry in manifest['sourceManifest'] if entry['path'] == 'packages/catalog/src/discovery/cursor.ts')
        fixture = {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in [ca, leaf, key]}
        report = {'upstreamCommit': manifest['upstreamCommit'], 'bunSha256': BUN_SHA, 'scriptSha256': hashlib.sha256(script.read_bytes()).hexdigest(), 'cursorSource': cursor_source, 'sourceRun': str(args.source_run.resolve()), 'fixtureSha256': fixture, 'clientConfigs': client_configs, 'clients': clients, 'server': json.loads(receipts.read_text()), 'scope': 'Actual unchanged Cursor source through Bun node:http2 and a real CA-signed H2 endpoint; not Node.js runtime parity.'}
        (run / 'receipt.json').write_text(json.dumps(report, indent=2) + '\n')
        summary = [{'name': client['name'], 'steps': [{'id': step['id'], 'models': step['models']} for step in client['steps']]} for client in clients]
        print(json.dumps({'receipt': str((run / 'receipt.json').resolve()), 'clients': summary, 'requests': len(report['server']['requests'])}))
    finally:
        server.terminate()
        server.wait(timeout=5)
        stderr.close()


if __name__ == '__main__':
    main()
