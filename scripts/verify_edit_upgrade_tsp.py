"""Actual normal ara binary + Windows ConPTY or Linux PTY controlled TSP peer.

This exercises protocol/process behavior only. It does not accept physical Tern
layout, theme fidelity, a live model or paid generation. No build is run here.
"""
from __future__ import annotations
import argparse, datetime, errno, hashlib, json, os, platform, re, select, shutil, signal, struct, subprocess, sys, threading, time
from pathlib import Path
if os.name == "nt":
    import windows_conpty_ctrl_c as console
else:
    console = None
import verify_snapcompact_host as host
ROOT=Path(__file__).resolve().parents[1]
def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def require(ok,message):
    if not ok:raise AssertionError(message)
class PosixPty:
    """One real Linux controlling PTY; the child owns its own session/group.

    util-linux setsid establishes the controlling terminal before exec without
    preexec_fn in this multithreaded HTTP harness. No shell command is built.
    """
    boundary = 'Linux openpty + setsid --ctty exec; raw master bytes'

    def __init__(self, argv, cwd, env, columns=80, rows=30):
        import fcntl
        import termios
        require(sys.platform.startswith('linux'), 'POSIX owner requires actual Linux')
        setsid = shutil.which('setsid')
        require(setsid is not None, 'util-linux setsid is required; no PTY fallback/skip')
        require(isinstance(argv, list) and argv, 'Linux child command must remain argv')
        require(1 <= columns <= 32767 and 1 <= rows <= 32767, 'invalid PTY size')
        self.raw = bytearray()
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.reader_error = None
        self.master, slave = os.openpty()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
            self.slave_name = os.ttyname(slave)
            require(os.isatty(slave) and os.isatty(self.master), 'openpty did not return terminals')
            self.process = subprocess.Popen([setsid, '--ctty', '--', *argv], cwd=cwd, env=env,
                stdin=slave, stdout=slave, stderr=slave, close_fds=True)
        except BaseException:
            os.close(self.master)
            raise
        finally:
            os.close(slave)
        os.set_blocking(self.master, False)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        try:
            while not self.stop.is_set():
                ready, _, _ = select.select([self.master], [], [], .1)
                if not ready:
                    continue
                try:
                    data = os.read(self.master, 65536)
                except BlockingIOError:
                    continue
                except OSError as error:
                    if error.errno in (errno.EIO, errno.EBADF):
                        break  # Linux PTY EOF after the final slave closes.
                    raise
                if not data:
                    break
                with self.lock:
                    self.raw.extend(data)
        except BaseException as error:
            if not (self.stop.is_set() and isinstance(error, OSError) and error.errno == errno.EBADF):
                self.reader_error = f'{type(error).__name__}: {error}'

    def send(self, data):
        remaining = memoryview(data)
        deadline = time.monotonic() + 5
        while remaining:
            require(self.process.poll() is None, 'PTY child exited before input write')
            require(time.monotonic() < deadline, 'PTY input write timed out')
            _, writable, _ = select.select([], [self.master], [], .1)
            if not writable:
                continue
            try:
                count = os.write(self.master, remaining)
            except BlockingIOError:
                continue
            require(count > 0, 'PTY input write made no progress')
            remaining = remaining[count:]

    def text(self):
        with self.lock:
            text = self.raw.decode('utf-8', 'replace')
        return re.sub(r'\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[=>]', '', text).replace('\r', '')

    def exit_code(self, timeout):
        try:
            return self.process.wait(timeout=timeout) if timeout else self.process.poll()
        except subprocess.TimeoutExpired:
            return None

    def _signal_group(self, number):
        try:
            os.killpg(self.process.pid, number)
        except ProcessLookupError:
            pass

    def close(self):
        # The PID is also the session/group ID established by setsid. Never
        # enumerate or signal unrelated processes; include owned descendants.
        self._signal_group(signal.SIGTERM)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            self.process.poll()
            try:
                os.killpg(self.process.pid, 0)
            except ProcessLookupError:
                break
            time.sleep(.025)
        self._signal_group(signal.SIGKILL)
        self.process.wait(timeout=2)
        self.reader.join(timeout=1)
        self.stop.set()
        os.close(self.master)
        self.reader.join(timeout=1)
        require(not self.reader.is_alive(), 'PTY raw reader did not stop')
        require(self.reader_error is None, f'PTY raw reader failed: {self.reader_error}')


def open_terminal(argv, work, env):
    if os.name == 'nt':
        return console.Pty(subprocess.list2cmdline(argv), str(work), env, columns=80, rows=30)
    return PosixPty(argv, str(work), env, columns=80, rows=30)


def platform_boundary():
    return ('Windows real ConPTY' if os.name == 'nt' else PosixPty.boundary)


class Peer:
    def __init__(self,pty,*,version=1):self.pty=pty;self.version=version;self.at=0;self.partial={};self.frames=[];self.records=[];self.auto_ack=True;self.surface=None;self.editor=None;self.pending_hello=None;self.hello_sent_at=None;self.late_transition=False;self.rows_before_hello=False;self.phase="spawn";self.stages=[]
    def send(self,value,verb='e',split=False):
        # Windows input crosses ConPTY's OSC877 relay; Linux uses fixed APC.
        prefix, end = ('\x1b]877;tsp;', '\x07') if os.name == 'nt' else ('\x1b_tsp;', '\x1b\\')
        data=(prefix+verb+';'+json.dumps(value,ensure_ascii=False,separators=(',',':'))+end).encode()
        if split:
            for byte in data:self.pty.send(bytes([byte]))
        else:self.pty.send(data)
    def step(self):
        with self.pty.lock:data=bytes(self.pty.raw)
        while True:
            start=data.find(b'\x1b_tsp;',self.at)
            if start<0:break
            end=data.find(b'\x1b\\',start)
            if end<0:break
            self.at=end+2;parts=data[start+6:end].decode('utf-8').split(';');verb=parts.pop(0);params={}
            while len(parts)>1 and re.fullmatch(r'[A-Za-z0-9_-]+=[\x21-\x3a\x3c-\x7e]*',parts[0]):
                key,value=parts.pop(0).split('=',1);params[key]=value
            body=';'.join(parts)
            if 'c' in params:
                identity=(verb,params['c']);body=self.partial.pop(identity,'')+body
                if params.get('m')=='1':self.partial[identity]=body;continue
            value=json.loads(body);self.records.append({'verb':verb,'value':value})
            if verb=='q' and value.get('q')=='hello':
                hello={'r':'hello','v':self.version,'term':'controlled-ConPTY-peer' if os.name == 'nt' else 'controlled-Linux-PTY-peer','kinds':['col','rows','text','tool','section','diff','code','row','icon','spinner','editor'],'features':['edit','undo','send'],'credits':1,'apc':53,'cols':80,'dark':True}
                if self.version==1 and self.hello_sent_at is None:self.pending_hello=(hello,self.at)
                else:self.send(hello,'r',split=True)
            elif verb=='o':
                self.surface=value['id']
                if self.hello_sent_at is not None and not self.late_transition:
                    transition=data[self.hello_sent_at:start];require(b'\r' in transition and b'\x1b[J' in transition,'late hello opened before erasing ANSI viewport');require(b'\x1b[3J' not in transition,'late hello cleared scrollback');self.late_transition=True
            elif verb=='f':
                self.frames.append(value)
                for op in value['ops']:
                    if op[0]=='add' and op[4].get('k')=='editor':self.editor=op[1]
                if self.auto_ack:self.send({'ev':'ack','sf':value['sf'],'s':value['s']})
        # Delay the first hello until this same real child has painted ANSI
        # prompt rows, proving Rows -> Native rather than only initial native.
        if self.pending_hello is not None:
            hello,after_query=self.pending_hello
            if b'> ' in data[after_query:]:
                self.rows_before_hello=True;self.hello_sent_at=len(data);self.pending_hello=None;self.send(hello,'r',split=True)
    def prompt_editor_after_close(self):
        # A surface close ends Run ownership; it does not publish the next
        # prompt's raw reader. Require its real reopened native editor frame.
        last_close = max((index for index, record in enumerate(self.records)
            if record['verb'] == 'x'), default=-1)
        after = self.records[last_close + 1:]
        return last_close >= 0 and any(record['verb'] == 'o' for record in after) and any(
            record['verb'] == 'f' and any(isinstance(op, list) and len(op) > 4
                and op[0] == 'add' and isinstance(op[4], dict)
                and op[4].get('k') == 'editor'
                and op[4].get('p', {}).get('role') == 'omp.prompt.editor'
                and op[4].get('p', {}).get('text') == ''
                for op in record['value'].get('ops', []))
            for record in after)
    def stage(self, label):
        self.phase = label
        self.stages.append({'label': label, 'atMonotonic': time.monotonic()})
    def wait(self,predicate,seconds=12,*,label):
        self.stage(label)
        until=time.monotonic()+seconds
        while time.monotonic()<until:
            self.step()
            if predicate():return
            require(getattr(self.pty, 'reader_error', None) is None, f'{self.phase}: raw reader failed');require(self.pty.exit_code(0) is None,f'{self.phase}: child exited before expected observation')
            time.sleep(.025)
        raise TimeoutError(f'{self.phase}: controlled peer observation timed out after {seconds}s; records={len(self.records)} frames={len(self.frames)} partialChunks={len(self.partial)}')
def run(trial,version):
    name='tsp-v1' if version==1 else 'unsupported-version'
    directory = trial.output / name
    result={'case':name,'status':'FAIL','platform':sys.platform,'ttyBoundary':platform_boundary(),
        'boundary':'normal binary/actual PTY/controlled loopback upstream; no physical Tern visual acceptance',
        'phase':'setup','failure':None,'childBinaryPath':str(trial.binary),'childBinarySha256':sha(trial.binary),
        'stdoutMode':'actual ConPTY VT output pipe' if os.name == 'nt' else 'actual PTY slave for stdin/stdout/stderr; raw master reader'}
    pty = peer = fake = None
    try:
        directory,work,session=trial.setup(name,[{'type':'message','id':'peer-seed','parentId':None,'timestamp':'2026-10-18T00:00:00.000Z','message':{'role':'user','content':'TSP_PROCESS_SENTINEL','timestamp':1}}])
        result['phase']='loopback-upstream'
        with host.remote.RemoteUpstream(directory,generic=True) as fake:
            models=host.configure(directory,fake);env=trial.environment(directory);env['ARA_TUI_NATIVE']='1';env.pop('TMUX',None);env.pop('STY',None)
            if os.name == 'posix':
                env['TERM']='xterm-256color'
            argv=host.command(trial,directory,work,session,models,repl=True)
            result['phase']='spawn-actual-pty';pty=open_terminal(argv,work,env);peer=Peer(pty,version=version)
            result['pid']=pty.process.pid if os.name == 'posix' else pty.pi.dwProcessId
            if os.name == 'posix':result['slaveTty']=pty.slave_name
            peer.wait(lambda:any(r['verb']=='q' for r in peer.records),label='hello-query-apc')
            if os.name == 'posix':
                import fcntl
                import termios
                result['processSessionId']=os.getsid(pty.process.pid)
                result['processGroupId']=os.getpgid(pty.process.pid)
                height,width,_,_=struct.unpack('HHHH',fcntl.ioctl(pty.master,termios.TIOCGWINSZ,b'\0'*8))
                result['ttyDimensions']={'columns':width,'rows':height}
                require(result['processSessionId']==pty.process.pid and result['processGroupId']==pty.process.pid,'PTY child must own its setsid session/process group')
                require((width,height)==(80,30),'actual PTY dimensions do not match requested viewport')
            if version==1:
                peer.wait(lambda:peer.editor is not None,label='late-hello-native-editor')
                require(not fake.requests,'terminal handshake called a model')
                require(peer.rows_before_hello and peer.late_transition,'real late Rows-to-Native transition was not observed')
                require(any(r['verb']=='o' for r in peer.records) and any(r['verb']=='t' for r in peer.records),'open/palette missing')
                peer.stage('typed-text-and-bracketed-paste');pty.send(b'draft');pty.send('\x1b[200~-🦀\x1b[201~'.encode())
                peer.wait(lambda:any('draft-🦀' in json.dumps(f,ensure_ascii=False) for f in peer.frames),label='native-draft-unicode-paste')
                peer.auto_ack=False
                before=len(peer.frames);pty.send(b'!');peer.wait(lambda:len(peer.frames)>before,label='first-frame-credit')
                peer.stage('credit-exhaustion-coalesces');before=len(peer.frames);pty.send(b'?');start=time.monotonic()
                while time.monotonic()-start<.3:peer.step();time.sleep(.025)
                require(len(peer.frames)==before,'credit exhaustion did not coalesce frames')
                peer.wait(lambda:len(peer.frames)>before,seconds=7,label='stalled-ack-recovery')
                result['stalledAckRecoverySeconds']=round(time.monotonic()-start,3)
                peer.auto_ack=True
                last=peer.frames[-1];peer.send({'ev':'ack','sf':last['sf'],'s':last['s']})
                # Current draft has UTF16 len10: draft + '-' + crab(2) + !?.
                peer.stage('peer-edit-utf16');peer.send({'ev':'edit','sf':peer.surface,'id':peer.editor,'from':0,'to':5,'text':'kept','cursor':4,'len':10},split=True)
                peer.wait(lambda:any('kept-🦀!?' in json.dumps(f,ensure_ascii=False) for f in peer.frames),label='native-edited-value')
                peer.send({'ev':'undo','sf':peer.surface,'id':peer.editor})
                peer.wait(lambda:any('draft-🦀!?' in json.dumps(f,ensure_ascii=False) for f in peer.frames),label='native-undo-value')
                peer.stage('resize-theme-motion-and-submit');peer.send({'ev':'resize','sf':peer.surface,'cols':40});peer.send({'ev':'theme','sf':peer.surface,'dark':False});peer.send({'ev':'motion','sf':peer.surface,'reduce':True})
                peer.send({'ev':'send','sf':peer.surface,'id':peer.editor,'text':'peer-submitted'},split=True)
                peer.wait(lambda:len(fake.requests)==1,seconds=20,label='single-controlled-provider-call')
                peer.wait(lambda:any('peer-submitted' in json.dumps(row,ensure_ascii=False) for row in host.shared.journal(session).values()),seconds=20,label='submitted-journal-source')
                # Every raw ownership cycle queries anew; prompt/run transitions.
                peer.wait(lambda:sum(r['verb']=='x' for r in peer.records)>=2,seconds=20,label='surface-close-ownership-cycles')
                result.update(frames=len(peer.frames),chunksObserved=any(len(json.dumps(r['value'],ensure_ascii=False).encode())>53 for r in peer.records),lateHelloViewportErasedBeforeOpen=peer.late_transition,nativeEdits=True,undo=True,submitJournal=True,closeCount=sum(r['verb']=='x' for r in peer.records))
            else:
                peer.wait(lambda:'> ' in pty.text(),label='unsupported-version-ansi-prompt');require(not any(r['verb']=='o' for r in peer.records),'unsupported version opened a surface');require(not fake.requests,'fallback called a model')
            if version == 1:
                peer.wait(lambda:peer.prompt_editor_after_close(), label='next-prompt-owner-before-eof')
                result['exitPromptReady'] = True
            # Exit via the same raw input owner: no extra user/model turn.
            peer.stage('raw-owner-eof');pty.send(b'\x15\x04');peer.wait(lambda:pty.exit_code(0) is not None,seconds=12,label='clean-eof-exit')
            require(pty.exit_code(0)==0,'EOF failed to restore/exit cleanly');require(not fake.errors,'fake route errors')
            result.update(status='PASS',exitCode=0,modelCalls=len(fake.requests))
    except Exception as error:
        result['status']='FAIL';result['failure']=f'{type(error).__name__}: {error}'
    finally:
        result['phase']=peer.phase if peer else result['phase']
        result['stages']=peer.stages if peer else []
        if pty is not None:
            try:pty.close()
            except Exception as error:
                result['status']='FAIL';result['cleanupFailure']=f'{type(error).__name__}: {error}'
            with pty.lock:data=bytes(pty.raw)
            (directory/'terminal.vt').write_bytes(data)
            result['terminalSha256']=hashlib.sha256(data).hexdigest()
            result['exitCode']=pty.exit_code(0) if os.name == 'posix' else result.get('exitCode')
        if peer is not None:
            (directory/'peer-records.json').write_text(json.dumps(peer.records,ensure_ascii=False,indent=2),encoding='utf-8')
            result.update(records=len(peer.records),partialChunks=len(peer.partial))
        if fake is not None:result['modelCalls']=len(fake.requests)
        directory.mkdir(parents=True,exist_ok=True)
        host.shared.dump(directory/'receipt.json',result)
    return result


def source_pins():
    # The normal production artifact comes from the frozen module build. Bind
    # all repository crate inputs and the actual transitive Python helpers too.
    paths={ROOT/'Cargo.toml',ROOT/'Cargo.lock',ROOT/'scripts/windows_conpty_ctrl_c.py',Path(__file__)}
    paths.update(path for path in (ROOT/'crates').rglob('*') if path.is_file())
    for module in tuple(sys.modules.values()):
        filename=getattr(module,'__file__',None)
        if filename:
            path=Path(filename).resolve()
            if path.is_relative_to(ROOT/'scripts') and path.is_file():paths.add(path)
    return {path.relative_to(ROOT).as_posix():sha(path) for path in sorted(paths)}


def runtime_pins(cargo_json):
    paths={'python':Path(sys.executable).resolve(),'cargoJson':cargo_json.resolve()}
    if os.name == 'posix':
        setsid=shutil.which('setsid');require(setsid is not None,'actual Linux setsid missing')
        paths['setsid']=Path(setsid).resolve()
    return {name:{'path':str(path),'sha256':sha(path)} for name,path in paths.items()}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--cargo-json',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    require(os.name in ('nt','posix'),'actual Windows ConPTY or Linux PTY required')
    require(os.name=='nt' or sys.platform.startswith('linux'),'POSIX acceptance is Linux only')
    binary=a.binary.resolve();require(binary.is_file(),'delivered normal binary missing')
    artifacts=[json.loads(line) for line in a.cargo_json.read_text(encoding='utf-8').splitlines() if line.strip()]
    require(any(r.get('reason')=='compiler-artifact' and r.get('target',{}).get('name')=='ara' and r.get('profile',{}).get('test') is False and Path(r.get('executable') or '.').resolve()==binary for r in artifacts),'normal production binary artifact receipt missing')
    output=a.output.resolve()/datetime.datetime.now(datetime.timezone.utc).strftime('run-%Y%m%dT%H%M%S.%fZ');output.mkdir(parents=True)
    trial=host.shared.Trial(binary,output,30);pins=source_pins();runtime=runtime_pins(a.cargo_json);before=sha(binary);started=time.monotonic();cases=[];failure=None;phase='case-setup'
    try:
        for version in [1,999]:
            phase='tsp-v1/setup' if version==1 else 'unsupported-version/setup'
            case=run(trial,version);cases.append(case);phase=f"{case['case']}/{case['phase']}"
            if case['status']!='PASS':
                failure=case['failure'] or case.get('cleanupFailure') or 'case failed';break
        if failure is None:require(len(cases)==2,'both original protocol cases must execute')
    except Exception as error:failure=f'{type(error).__name__}: {error}'
    unchanged=sha(binary)==before and all(sha(ROOT/name)==value for name,value in pins.items()) and all(sha(value['path'])==value['sha256'] for value in runtime.values())
    receipt={'status':'PASS' if failure is None and unchanged else 'FAIL','seconds':time.monotonic()-started,'platform':sys.platform,'osName':os.name,'machine':platform.machine(),'kernel':platform.release(),'pythonVersion':sys.version,'ttyBoundary':platform_boundary(),'binaryPath':str(binary),'binarySha256':before,'runtimePins':runtime,'pythonHelperFiles':[{'path':str(ROOT/name),'sha256':value} for name,value in pins.items() if name.startswith('scripts/')],'stdoutMode':'actual ConPTY VT output pipe' if os.name == 'nt' else 'actual PTY slave for stdin/stdout/stderr; raw master reader','sourcePins':pins,'sourceUnchanged':unchanged,'cases':cases,'failure':failure,'failurePhase':phase if failure else None,'unverified':['physical Tern visual/theme/keyboard','live provider/model','other platform PTY (separate real receipts required)','tmux process behavior (source/unit separately)']}
    host.shared.dump(output/'receipt.json',receipt);print(json.dumps({'status':receipt['status'],'receipt':str(output/'receipt.json'),'platform':sys.platform,'failurePhase':receipt['failurePhase'],'failure':failure}));return 0 if receipt['status']=='PASS' else 1
if __name__=='__main__':raise SystemExit(main())
