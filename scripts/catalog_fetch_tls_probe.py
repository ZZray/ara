"""Real Bun fetch CA override, mTLS, peer-name and cipher probes."""
import argparse,datetime,hashlib,json,os,pathlib,shutil,subprocess,time,uuid
p=argparse.ArgumentParser();p.add_argument('--source-run',type=pathlib.Path,required=True);p.add_argument('--bun',type=pathlib.Path,required=True);p.add_argument('--openssl',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--supplemental',action='store_true');a=p.parse_args()
manifest=json.loads((a.source_run/'source-manifest.json').read_text());assert manifest['upstreamCommit']=='596f2da7101178214aa27a753529d15e6b7ad91d'
for entry in [*manifest['sourceManifest'],*manifest['licenses']]:
 data=(a.source_run/'upstream'/entry['path']).read_bytes();assert hashlib.sha256(data).hexdigest()==entry['sha256'];assert hashlib.sha1(f'blob {len(data)}\0'.encode()+data).hexdigest()==entry['gitBlob']
r=a.output/('run-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-'+uuid.uuid4().hex[:8]);r.mkdir(parents=True)
script=r/'probe.mjs';shutil.copyfile(pathlib.Path(__file__).with_suffix('.mjs'),script)
def run(*args):
 c=subprocess.run([str(a.openssl),*map(str,args)],capture_output=True,timeout=15);c.check_returncode()
def cert(name,ca=None,san='DNS:localhost,IP:127.0.0.1',client=False):
 pem,key,csr=[r/(name+suffix) for suffix in ['.pem','.key','.csr']]
 if ca is None:run('req','-x509','-newkey','rsa:2048','-nodes','-days','2','-subj','/CN='+name,'-addext','basicConstraints=critical,CA:TRUE','-keyout',key,'-out',pem)
 else:
  run('req','-new','-newkey','rsa:2048','-nodes','-subj','/CN='+name,'-keyout',key,'-out',csr)
  ext=r/(name+'.cnf');ext.write_text('subjectAltName='+san+'\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage='+('clientAuth' if client else 'serverAuth')+'\n')
  run('x509','-req','-days','2','-in',csr,'-CA',ca[0],'-CAkey',ca[1],'-CAcreateserial','-out',pem,'-extfile',ext)
 return str(pem.resolve()),str(key.resolve())
ca=cert('root-a');other=cert('root-b');leaf=cert('server-a',ca);leaf_b=cert('server-b',other);named=cert('named-server',ca,'DNS:fixture.test');client=cert('client-b',other,client=True)
servers={n:{'cert':v[0],'key':v[1]} for n,v in [('a',leaf),('b',leaf_b),('named',named),('mtls',leaf),('cipher',leaf)]};servers['mtls']['ca']=other[0];servers['cipher']['ciphers']='ECDHE-RSA-AES128-GCM-SHA256'
if a.supplemental:servers['tls13']={'cert':leaf[0],'key':leaf[1],'minVersion':'TLSv1.3','maxVersion':'TLSv1.3'}
ready=r/'ready.json';receipts=r/'server-receipts.json';cfg=r/'server.json';cfg.write_text(json.dumps({'servers':servers,'ready':str(ready.resolve()),'receipts':str(receipts.resolve())}))
env=os.environ.copy();env.pop('NODE_EXTRA_CA_CERTS',None);stderr=(r/'server.stderr').open('wb');server=subprocess.Popen([str(a.bun),str(script.resolve()),'--server',str(cfg.resolve())],env=env,stdout=subprocess.DEVNULL,stderr=stderr)
try:
 start=time.monotonic()
 while not ready.exists():
  if server.poll() is not None:raise RuntimeError('server stopped')
  if time.monotonic()-start>5:raise RuntimeError('server readiness')
  time.sleep(.01)
 ports=json.loads(ready.read_text());cases=[]
 def add(id,target='a',tls=None,startup=None,wrapper=False):cases.append({'id':id,'url':f'https://127.0.0.1:{ports[target]}/'+id,'tls':tls or {},'startup':startup,'wrapper':wrapper,'source':str((a.source_run/'upstream/packages/utils/src/tls-fetch.ts').resolve())})
 empty=r/'empty.pem';empty.write_text('');bad=r/'bad.pem';bad.write_text('not a certificate');add('startup-a-curated-null',tls={'ca':None},startup=ca[0]);add('startup-a-curated-empty-string',tls={'ca':str(empty.resolve())},startup=ca[0]);add('startup-a-curated-empty-member',tls={'ca':[str(empty.resolve())]},startup=ca[0]);add('startup-a-curated-invalid',tls={'ca':[str(bad.resolve())]},startup=ca[0]);add('unset-no-ca');add('startup-a-no-ca',startup=ca[0]);add('startup-a-curated-b-reject-a',tls={'ca':[other[0]]},startup=ca[0]);add('startup-a-curated-b-trust-b','b',{'ca':[other[0]]},ca[0]);add('startup-a-curated-empty',tls={'ca':[]},startup=ca[0]);add('unset-curated-a',tls={'ca':[ca[0]]});add('unset-reject-false',tls={'rejectUnauthorized':False});add('curated-b-reject-false',tls={'ca':[other[0]],'rejectUnauthorized':False});add('named-no-override','named',{'ca':[ca[0]]});add('named-serverName','named',{'ca':[ca[0]],'serverName':'fixture.test'});add('named-servername-lowercase','named',{'ca':[ca[0]],'servername':'fixture.test'});add('mtls-no-client','mtls',{'ca':[ca[0]]});add('mtls-valid-client','mtls',{'ca':[ca[0]],'cert':client[0],'key':client[1]});add('mtls-cert-only','mtls',{'ca':[ca[0]],'cert':client[0]});add('cipher-matching','cipher',{'ca':[ca[0]],'ciphers':'ECDHE-RSA-AES128-GCM-SHA256'});add('cipher-disjoint','cipher',{'ca':[ca[0]],'ciphers':'ECDHE-RSA-AES256-GCM-SHA384'});add('cipher-invalid','cipher',{'ca':[ca[0]],'ciphers':'NOT-A-CIPHER'});add('wrapped-startup-a-curated-b-a',tls={'ca':[other[0]]},startup=ca[0],wrapper=True);add('wrapped-startup-a-curated-b-b','b',{'ca':[other[0]]},ca[0],True)
 if a.supplemental:
  cases.clear()
  add('ordinary-cert-only',tls={'ca':[ca[0]],'cert':client[0]});add('ordinary-key-only',tls={'ca':[ca[0]],'key':client[1]});add('ordinary-invalid-cert-only',tls={'ca':[ca[0]],'cert':str(bad.resolve())});add('ordinary-invalid-key-only',tls={'ca':[ca[0]],'key':str(bad.resolve())})
  add('tls13-default','tls13',{'ca':[ca[0]]});add('tls13-with-tls12-aes128','tls13',{'ca':[ca[0]],'ciphers':'ECDHE-RSA-AES128-GCM-SHA256'});add('tls13-with-tls12-aes256','tls13',{'ca':[ca[0]],'ciphers':'ECDHE-RSA-AES256-GCM-SHA384'});add('tls13-invalid-cipher','tls13',{'ca':[ca[0]],'ciphers':'NOT-A-CIPHER'})
 results=[]
 for case in cases:
  file=r/(case['id']+'.json');file.write_text(json.dumps(case));e=env.copy()
  if case['startup'] is not None:e['NODE_EXTRA_CA_CERTS']=case['startup']
  c=subprocess.run([str(a.bun),str(script.resolve()),'--client',str(file.resolve())],env=e,capture_output=True,timeout=8);(r/(case['id']+'.stdout')).write_bytes(c.stdout);(r/(case['id']+'.stderr')).write_bytes(c.stderr);c.check_returncode();results.append(json.loads(c.stdout))
 manifest=json.loads((a.source_run/'source-manifest.json').read_text());source=next(v for v in manifest['sourceManifest'] if v['path']=='packages/utils/src/tls-fetch.ts');report={'upstreamCommit':manifest['upstreamCommit'],'bunSha256':hashlib.sha256(a.bun.read_bytes()).hexdigest(),'sourceRun':str(a.source_run),'source':source,'scriptSha256':hashlib.sha256(script.read_bytes()).hexdigest(),'driverSha256':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),'fixtureSha256':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in r.iterdir() if p.suffix in ['.pem','.key','.cnf']},'caseConfigSha256':{case['id']:hashlib.sha256((r/(case['id']+'.json')).read_bytes()).hexdigest() for case in cases},'cases':results,'serverReceipts':json.loads(receipts.read_text()) if receipts.exists() else [],'scope':'Real Bun 1.4.0 global fetch plus unchanged source wrapper; no native acceptance claim'};(r/'receipt.json').write_text(json.dumps(report,indent=2));print(json.dumps({'receipt':str((r/'receipt.json').resolve()),'cases':[{'id':v['id'],'ok':v['ok'],'code':v.get('code')} for v in results]}))
finally:server.terminate();server.wait(timeout=5);stderr.close()
