import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
root = str(Path(sys.argv[2]).resolve())
output = Path(sys.argv[3])
output.parent.mkdir(parents=True, exist_ok=True)
source_commit = subprocess.check_output(['git', '-C', root, 'rev-parse', 'HEAD'], text=True).strip()
source_diff = subprocess.check_output(['git', '-C', root, 'diff', '--binary', 'HEAD'])
source_status = subprocess.check_output(['git', '-C', root, 'status', '--short'], text=True).strip()
with tempfile.TemporaryDirectory(prefix='code-kb-rss-telemetry-') as telemetry:
    environment = dict(os.environ, CODE_KB_TELEMETRY_DIR=telemetry)
    log = output.with_suffix('.stderr.log').open('w')
    child = subprocess.Popen([binary, 'serve', '--root', root], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True, env=environment)
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)
    identifier = 0
    def request(method, params):
        global identifier
        identifier += 1
        child.stdin.write(json.dumps({'jsonrpc':'2.0','id':identifier,'method':method,'params':params})+'\n')
        child.stdin.flush()
        if not selector.select(60):
            raise RuntimeError('MCP response exceeded 60 seconds')
        response = json.loads(child.stdout.readline())
        if response.get('error') or response.get('result',{}).get('isError'):
            raise RuntimeError(response)
        return response
    def memory():
        time.sleep(0.05)
        values = {}
        for line in Path(f'/proc/{child.pid}/smaps_rollup').read_text().splitlines():
            if ':' in line:
                key,value=line.split(':',1)
                if key in ('Rss','Pss','Anonymous'):
                    values[key+'_mb']=round(int(value.split()[0])/1024,2)
        for line in Path(f'/proc/{child.pid}/status').read_text().splitlines():
            if line.startswith('VmHWM:'):
                values['Peak_Rss_mb'] = round(int(line.split()[1]) / 1024, 2)
        return values
    try:
        request('initialize', {'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'cross-language-rss','version':'1'}})
        initial=memory()
        workloads=[('file_skeleton',{'file_path':'crates/code-kb-core/src/edit_check.rs'}),('lookup_symbol',{'query':'parameter named argument'}),('get_symbol_context',{'symbol_name':'search_symbols_scoped','file_path':'crates/code-kb-core/src/queries.rs'}),('find_references',{'symbol_name':'search_symbols_scoped','file_path':'crates/code-kb-core/src/queries.rs','limit':200}),('blast_radius',{'symbol':'find_callee_signatures','limit':20})]
        rows=[]
        for tool,args in workloads:
            times=[]
            for _ in range(4):
                started=time.perf_counter()
                response=request('tools/call',{'name':tool,'arguments':dict(args,project_root=root)})
                times.append(round((time.perf_counter()-started)*1000,2))
            rows.append({'tool':tool,'args':args,'warm_ms':times[1:],'response_bytes':len(json.dumps(response)),'memory':memory()})
        result={'source_commit':source_commit,'source_diff_sha256':hashlib.sha256(source_diff).hexdigest(),'source_status':source_status,'probe_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'binary':binary,'binary_sha256':hashlib.sha256(Path(binary).read_bytes()).hexdigest(),'root':root,'initial':initial,'calls':rows,'final':memory()}
        output.parent.mkdir(parents=True,exist_ok=True)
        output.write_text(json.dumps(result,indent=2)+'\n')
        print(json.dumps({'binary':binary,'final':result['final'],'output':str(output)}))
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        selector.close()
        log.close()
