"""Actual same-EXE terminal workers: shell input and bounded failure cleanup.
Isolated Host contract fixtures, not public authentication or model acceptance.
"""
import argparse,json,os,socket,struct,tempfile,time
from pathlib import Path
from verify_jobs import Jobs,atomic_json,compact,sha,until,require,read_receipt

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',required=True,type=Path);p.add_argument('--output',type=Path);a=p.parse_args()
 root=a.output.resolve() if a.output else Path(tempfile.mkdtemp(prefix='hw-term-worker-'));root.mkdir(parents=True,exist_ok=True);require(not any(root.iterdir()),'Use fresh evidence')
 jobs=Jobs(a.binary,root);result={'status':'FAIL','passed':[],'scope':'actual terminal workers; isolated contracts'}
 def prepare(name,script,language='python',**limits):
  d=jobs.prepare(name,script,**limits);r=json.loads((d/'request.json').read_text());r['spec']['language']=language
  r['spec'].update(interactive=True,terminal=True,rows=24,cols=80);r['input_hash']=sha(compact(r['spec']).encode());atomic_json(d/'request.json',r)
  v=read_receipt(d);v.update(interactive=True,terminal=True,rows=24,cols=80);atomic_json(d/'receipt.json',v);return d
 def send(d,text):
  e=json.loads((d/'input-endpoint.json').read_text());r=json.loads((d/'request.json').read_text());message={'worker_nonce':e['worker_nonce'],'host_lease_expires_at':r['lease_expires_at'],'input_id':'shell-once','text':text,'eof':False}
  b=compact(message).encode()
  with socket.create_connection(('127.0.0.1',e['port']),timeout=5) as s:
   s.sendall(struct.pack('>I',len(b))+b)
   def read(n):
    b=b''
    while len(b)<n:
     q=s.recv(n-len(b));require(q,'Missing worker ACK');b+=q
    return b
   ack=json.loads(read(struct.unpack('>I',read(4))[0]));require(ack.get('receipt',{}).get('state')=='written',f'Shell input failed {ack}')
 try:
  cases=[('powershell',"$v=Read-Host 'HW_PS_READY'; [IO.File]::WriteAllText((Join-Path (Get-Location) 'ps-value.txt'),$v); Write-Output 'PS_DONE'",'HW_PS_READY','ps-value.txt'),('cmd','set /p "value=HW_CMD_READY:"\necho %value%>cmd-value.txt','HW_CMD_READY','cmd-value.txt')] if os.name=='nt' else [('sh',"printf 'HW_SH_READY:'; IFS= read -r value; printf '%s' \"$value\" > sh-value.txt",'HW_SH_READY','sh-value.txt')]
  for language,script,prompt,name in cases:
   d=prepare(language,script,language);jobs.spawn(d)
   until(lambda:(d/'input-endpoint.json').exists())
   until(lambda:(d/'stdout.bin').exists() and prompt in (d/'stdout.bin').read_text(encoding='utf-8',errors='replace'))
   send(d,'terminal-shell-value\r');r=jobs.terminal(d)
   require(r['state']=='succeeded' and (root/name).read_text().strip()=='terminal-shell-value',f'{language} shell effect missing {r}')
   result['passed'].append(language+'_actual_interactive_input')
  for label,limits,expected in [('timeout',{'timeout':1},'timed_out'),('lease',{'lease_seconds':2},'lease_expired'),('quota',{'quota':1024},'log_limit')]:
   script="import time\nprint('x'*10000,flush=True)\ntime.sleep(20)\n" if label=='quota' else "import time\ntime.sleep(20)\n"
   d=prepare(label,script,**limits);jobs.spawn(d);r=jobs.terminal(d);require(r['state']==expected and r['effects_unknown'],f'{label} wrong terminal receipt {r}')
   result['passed'].append('terminal_'+label+'_cleanup')
  for label in ['input-fault','output-fault']:
   d=prepare(label,'import time\ntime.sleep(20)\n')
   (d/('inputs.json' if label=='input-fault' else 'stdout.bin')).write_text('{invalid')
   t=time.monotonic();jobs.spawn(d);r=jobs.terminal(d,8)
   require(r['state']=='interrupted' and r['effects_unknown'] and time.monotonic()-t<8,f'{label} not supervised')
   result['passed'].append('terminal_'+label+'_interrupted_without_lease_wait')
  result['status']='PASS'
 finally:
  jobs.close();result['receipts']=jobs.receipts;(root/'result.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
 print(json.dumps({k:v for k,v in result.items() if k!='receipts'}))
if __name__=='__main__':main()
