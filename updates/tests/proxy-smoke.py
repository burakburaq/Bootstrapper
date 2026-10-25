"""Real HAProxy atomic map traffic through a stable Caddy frontend."""
import json
import pathlib
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

origin='http://127.0.0.1:18434'
route=pathlib.Path(sys.argv[1])
docker_config=sys.argv[2]
errors=[]; responses=[]; stop=threading.Event()
def live():
    with urllib.request.urlopen(origin+'/ops/live',timeout=10) as response:
        assert response.status==200
        return json.load(response)['instance']
def traffic():
    while not stop.is_set():
        try:
            instance=live()
            assert instance in ('container-test','container-second'),instance
            responses.append(instance)
        except Exception as error: errors.append(str(error))
        time.sleep(.03)
def runtime(command):
    return subprocess.run(['docker','--config',docker_config,'exec','updates-tests-router','sh','-c','printf "%s\\n" "$1" | nc -w 2 127.0.0.1 9999','sh',command],capture_output=True,text=True)
def switch(color):
    route.write_text(f'active {color}\n',encoding='utf-8')
    result=runtime(f'set map /etc/haproxy/maps/active.map active {color}')
    assert result.returncode==0 and not result.stdout.strip(),result.stderr+result.stdout
thread=threading.Thread(target=traffic);thread.start()
try:
    for color,instance in [('green','container-second'),('blue','container-test')]*10:
        switch(color)
        assert live()==instance
        time.sleep(.05)
    rejected=runtime('set map /etc/haproxy/maps/missing.map active green')
    assert rejected.stdout.strip(),'invalid map command was accepted'
    assert live()=='container-test'
    if '--drain' in sys.argv:
        # Hold an actual DB query in blue while routing new requests to green.
        lock=subprocess.Popen(['docker','--config',docker_config,'exec','updates-tests-postgres-1','psql','-U','updates_test','-d','updates_test','-c','BEGIN; LOCK TABLE update_releases IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(8); COMMIT;'],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        for _ in range(30):
            probe=subprocess.run(['docker','--config',docker_config,'exec','updates-tests-postgres-1','psql','-U','updates_test','-d','updates_test','-Atc',"SELECT count(*) FROM pg_locks WHERE relation='update_releases'::regclass AND mode='AccessExclusiveLock' AND granted"],capture_output=True,text=True)
            if probe.stdout.strip()=='1': break
            time.sleep(.05)
        else: raise AssertionError('database lock not acquired')
        result=[]
        def blocked_query():
            try:
                urllib.request.urlopen(origin+'/v1/tauri/alpha/production/windows-x86_64/1.0.0',timeout=20)
            except urllib.error.HTTPError as error:
                result.append((error.code,json.loads(error.read())))
            except Exception as error: result.append(str(error))
        blocked=threading.Thread(target=blocked_query); blocked.start()
        for _ in range(30):
            with urllib.request.urlopen('http://127.0.0.1:18433/health/inflight',timeout=10) as response:
                if json.load(response)['inflight']>0: break
            time.sleep(.05)
        else: raise AssertionError('request did not enter old server')
        switch('green'); assert live()=='container-second'
        shutdown=subprocess.Popen(['docker','--config',docker_config,'stop','--timeout','-1','updates-tests-server-1'],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        assert lock.wait(timeout=15)==0
        blocked.join(timeout=15)
        assert result and result[0][0]==503 and result[0][1]['code']=='storage_or_catalog_unavailable',result
        assert shutdown.wait(timeout=15)==0,'old server did not drain gracefully'
        print('In-flight DB request completed during atomic switch and graceful old-server shutdown')
    try: urllib.request.urlopen(origin+'/health/live',timeout=10); raise AssertionError('internal health exposed')
    except urllib.error.HTTPError as error: assert error.code==404
finally:
    stop.set();thread.join()
assert len(responses)>10,len(responses)
assert not errors,errors
print(f'Atomic proxy switching passed: 20 switches, {len(responses)} continuous HTTP responses, zero errors, invalid map retained live route')
