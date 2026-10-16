"""Actual Node/Hub/native terminal control, with a deterministic provider.
Proves integration and actual child effects, not real-model intelligence.
"""
import argparse
import json
from pathlib import Path
import tempfile
import threading
from verify_context import ContextFixture
from verify_host import require, stop
from verify_long_goals import LongHost, until

SCRIPT = """import os,sys,json,signal
from pathlib import Path
print('TTY:'+str([sys.stdin.isatty(),sys.stdout.isatty(),sys.stderr.isatty()]),flush=True)
# Request cursor position immediately, before any browser renderer attaches.
import time,select
reply=''
if os.name=='nt':
 import msvcrt
 sys.stdout.write('\x1b[6n');sys.stdout.flush()
 end=time.monotonic()+5
 while time.monotonic()<end and not reply.endswith('R'):
  if msvcrt.kbhit():reply+=msvcrt.getwch()
  else:time.sleep(.01)
else:
 import termios,tty
 old=termios.tcgetattr(0)
 try:
  tty.setraw(0)
  os.write(1,b'\x1b[6n')
  end=time.monotonic()+5
  while time.monotonic()<end and not reply.endswith('R'):
   if select.select([0],[],[],.1)[0]:reply+=os.read(0,1).decode()
 finally:termios.tcsetattr(0,termios.TCSANOW,old)
Path('terminal-query.txt').write_text(reply,encoding='utf-8')
assert reply.startswith('\x1b[') and reply.endswith('R'),repr(reply)
print('READY_LINE',flush=True)
a=input()
t=os.get_terminal_size()
Path('terminal-line.json').write_text(json.dumps({'a':a,'pid':os.getpid(),'cols':t.columns,'rows':t.lines}),encoding='utf-8')
print('READY_INTERRUPT',flush=True)
try:
 input()
except KeyboardInterrupt:
 Path('terminal-interrupt.txt').write_text('caught')
print('READY_EOF',flush=True)
try:
 b=input()
except EOFError:
 Path('terminal-eof.txt').write_text('eof')
print('TERMINAL_DONE',flush=True)
"""

class TerminalFixture(ContextFixture):
 def reply(self, handler, body):
  if not body.get('tools'): return super().reply(handler,body)
  self.agent_requests.append(body)
  tools={t['function']['name'] for t in body['tools']}
  require({'job_terminal_resize','job_terminal_signal'} <= tools,'Native terminal tools missing')
  jobs=[m for m in body['messages'] if m.get('role')=='tool' and '"worker_pid"' in str(m.get('content',''))]
  if not jobs:
   self.emit(handler,tool='job_start',arguments={'op_id':'terminal-once','language':'python','terminal':True,'rows':24,'cols':80,'timeout':90,'script':SCRIPT})
  else:
   require((self.root/'terminal-eof.txt').exists(),'Model resumed before terminal settled')
   self.emit(handler,text='TERMINAL_EFFECTS_VERIFIED')

class TerminalHost(LongHost):
 def __init__(self,args,root):
  super().__init__(args,root)
  self.fixture.server.shutdown();self.fixture.server.server_close()
  self.fixture=TerminalFixture(self.work)

def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--binary',required=True,type=Path);p.add_argument('--hub-binary',required=True,type=Path);p.add_argument('--output',type=Path)
 args=p.parse_args();root=args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix='hw-terminal-'))
 root.mkdir(parents=True,exist_ok=True);require(not any(root.iterdir()),'Use fresh evidence directory')
 host=TerminalHost(args,root);result={'passed':[],'scope':'actual native terminal/Node/Hub; controlled provider'}
 seq=0;gid='terminal-goal-24';key='terminal/session'
 def command(action,**fields):
  nonlocal seq
  seq+=1
  packet={'k':'job-command','command_id':f'terminal-{seq}','goal_id':gid,'session_key':key,'fp':host.device['fp'],'job_action':action,**fields}
  host.controller.send(packet)
  return host.controller.until(lambda m:m.get('k')=='job-command-result' and m.get('command_id')==packet['command_id'])
 def ok(action,**fields):
  r=command(action,**fields);require(r.get('job_ok') and not r.get('err'),f'{action} rejected: {r}');return r['job_data']
 def log(): return ok('log',job_id=job,job_stream='stdout',job_limit=32768).get('text','')
 timer=threading.Timer(110,lambda:(stop(host.node),stop(host.hub)));timer.daemon=True;timer.start()
 try:
  host.start();host.begin(gid,key,'TERMINAL_INTEGRATION_24 use one actual terminal for line input interrupt and EOF',jobs=True)
  until(lambda:(g if (g:=host.goal(gid)) and g['status']=='waiting_job' else None))
  host.controller.send({'k':'goal-inspect','fp':host.device['fp'],'session_key':key})
  host.controller.until(lambda m:m.get('k')=='goal-state' and m.get('goal',{}).get('status')=='waiting_job')
  jobs=ok('list')['jobs'];require(len(jobs)==1 and jobs[0]['terminal'] and jobs[0]['interactive'],'Terminal receipt missing')
  job=jobs[0]['job_id'];worker=jobs[0]['worker_pid']
  until(lambda:'READY_LINE' in log());require('TTY:[True, True, True]' in log(),'Child sees redirected streams')
  calls=len(host.fixture.agent_requests)
  resize=ok('resize',job_id=job,job_input_id='resize-103x37',job_rows=37,job_cols=103)
  require(resize['state']=='written' and resize['kind']=='resize','Resize not ACKed')
  require(ok('resize',job_id=job,job_input_id='resize-103x37',job_rows=37,job_cols=103)==resize,'Resize duplicate differs')
  ok('input',job_id=job,job_input_id='terminal-line',job_text='hello-terminal-24\r')
  until(lambda:(host.work/'terminal-line.json').exists())
  line=json.loads((host.work/'terminal-line.json').read_text());require(line['a']=='hello-terminal-24' and line['cols']==103 and line['rows']==37,f'Actual resize/input wrong: {line}')
  require(ok('status',job_id=job)['cols']==103,'Resize metadata not updated')
  require((host.work/'terminal-query.txt').read_text().endswith('R'),'Headless cursor query not answered')
  result['passed'].append('actual_tty_headless_cursor_query_merged_stream_input_and_native_resize_idempotency')
  host.controller.sock.close();host.controller=None;host.reconnect()
  current=ok('status',job_id=job);require(current['worker_pid']==worker and current['process_pid']==line['pid'],'Reconnect replaced process')
  require(command('signal',job_id=job,job_input_id='wrong-owner',job_signal='interrupt',session_key='other/session').get('err'),'Wrong scope signal admitted')
  until(lambda:'READY_INTERRUPT' in log())
  sig=ok('signal',job_id=job,job_input_id='interrupt-once',job_signal='interrupt')
  require(sig['state']=='written','Interrupt ACK missing')
  until(lambda:(host.work/'terminal-interrupt.txt').exists())
  require(ok('signal',job_id=job,job_input_id='interrupt-once',job_signal='interrupt')==sig,'Ctrl+C repeated')
  until(lambda:'READY_EOF' in log())
  require(len(host.fixture.agent_requests)==calls,'Terminal input caused model polling')
  eof=ok('signal',job_id=job,job_input_id='eof-once',job_signal='eof');require(eof['state']=='written','EOF not written')
  until(lambda:(host.work/'terminal-eof.txt').exists())
  until(lambda:host.goal(gid)['status']=='awaiting_review')
  status=ok('status',job_id=job);require(status['state']=='succeeded' and status['stdin_closed'],f'Unsettled terminal {status}')
  require(ok('log',job_id=job,job_stream='stderr',job_limit=8192)['text']=='','Terminal stream duplicated')
  require(command('signal',job_id=job,job_input_id='late',job_signal='interrupt').get('err'),'Late terminal signal admitted')
  require(not host.fixture.errors,str(host.fixture.errors))
  result['passed'].append('same_child_after_reconnect_scoped_ctrl_c_eof_and_no_model_polling')
  result.update(status='PASS',provider_calls=len(host.fixture.agent_requests),child_pid=line['pid'],receipt=status)
 finally:
  timer.cancel();host.close();(root/'result.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
 print(json.dumps(result))

if __name__=='__main__':main()
