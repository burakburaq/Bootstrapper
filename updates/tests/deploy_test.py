"""Read-only scenario harness for the actual Linux deploy.sh. Never runs Docker."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

MOCK = r'''
import json, sys
from pathlib import Path
root = Path.cwd()
path = root / 'control.json'
state = json.loads(path.read_text())
args = sys.argv[1:]
tool = Path(sys.argv[0]).name
with (root/'calls.jsonl').open('a') as log:
    log.write(json.dumps([tool,*args])+'\n')
scenario = state['scenario']
def save(): path.write_text(json.dumps(state))
if tool == 'docker':
    if 'inspect' in args: print('10.0.0.2'); sys.exit(0)
    command = next((x for x in args if x in ['ps','exec','run','up','pull','stop']), '')
    if command == 'ps':
        slot = args[-1]
        if slot == 'proxy' and state['active']: print('proxy-id')
        elif slot.removeprefix('update-server-') in state['running_slots']: print('slot-id')
    elif command == 'run' and scenario == 'migration-fail': sys.exit(7)
    elif command == 'exec' and 'router' in args:
        operation=args[-1]
        if operation.startswith('show map'): print(f"0x123 active {state['active']}")
        elif operation.startswith('set map'):
            destination=operation.split()[-1]
            if scenario == 'proxy-fail' and destination == 'green': print('map update rejected'); sys.exit(0)
            state['active']=destination; save()
    elif command == 'exec' and 'psql' in args and '-Atc' in args: print('1')
    elif command == 'exec' and 'smoke' in args:
        if scenario == 'smoke-before' and state['active']=='blue': sys.exit(11)
        if scenario == 'smoke-after' and state['active']=='green': sys.exit(11)
    elif command == 'up' and args[-1]=='proxy':
        route=(root/'state/routing/active.map').read_text()
        state['active']='green' if 'green' in route else 'blue'; save()
    elif command == 'up' and args[-1].startswith('update-server-'):
        state['running_slots'].append(args[-1].removeprefix('update-server-')); save()
    elif command == 'stop':
        assert args[-3:-1]==['--timeout','-1'], 'forced stop used'
        state['stopped'].append(args[-1]); save()
elif tool == 'curl':
    url=args[-1]
    slot='blue' if ':18081/' in url else 'green' if ':18082/' in url else state['active']
    if scenario=='health-after' and slot=='green' and state['active']=='green': sys.exit(22)
    if url.endswith('/health/inflight'): print(json.dumps({'inflight':5 if scenario=='drain-busy' else 0}))
    else: print(json.dumps({'ready':True,'instance':slot,'version':'0.1.0'}))
'''

@unittest.skipUnless(sys.platform.startswith('linux'), 'Linux jq/flock deployment harness; no Docker required')
class DeploymentTests(unittest.TestCase):
    def setUp(self):
        for tool in ('bash','jq','flock','sha256sum'):
            if not shutil.which(tool): self.skipTest(f'{tool} required')
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copy(Path(__file__).parents[1]/'deploy/deploy.sh', self.root/'deploy.sh')
        (self.root/'config.json').write_text(json.dumps({'public_url':'https://updates.example.test/'}))
        (self.root/'deploy.env').touch()
        (self.root/'compose.yml').touch()
        (self.root/'state/routing').mkdir(parents=True)
        (self.root/'state/routing/active.map').write_text('active blue\n')
        self.bin = self.root/'bin'; self.bin.mkdir()
        for tool in ('docker','curl'):
            path=self.bin/tool
            path.write_text(f'#!{sys.executable}\n'+MOCK)
            path.chmod(0o755)
        self.env=dict(os.environ, PATH=str(self.bin)+os.pathsep+os.environ['PATH'], UPDATE_OBSERVATION_SECONDS='0', UPDATE_DRAIN_SECONDS='12')
        self.control('success')
    def control(self, scenario, active='blue'):
        (self.root/'control.json').write_text(json.dumps({'scenario':scenario,'active':active,'stopped':[],'running_slots':[active] if active else []}))
    def state(self): return json.loads((self.root/'control.json').read_text())
    def calls(self): return [json.loads(x) for x in (self.root/'calls.jsonl').read_text().splitlines()]
    def run_deploy(self, mode='deploy', image=None):
        return subprocess.run(['bash',str(self.root/'deploy.sh'),mode,image or 'ghcr.io/example/updates@sha256:'+'a'*64],env=self.env,capture_output=True,text=True,timeout=30)
    def test_success_drains_only_old_updater(self):
        result=self.run_deploy(); self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.state()['active'],'green'); self.assertEqual(self.state()['stopped'],['update-server-blue'])
    def test_authorization_overlay_is_preserved_for_every_compose_operation(self):
        (self.root/'secrets').mkdir()
        (self.root/'secrets/install-authorization.key').write_text('a'*64)
        self.env['UPDATE_INSTALL_AUTHORIZATION']='1'
        result=self.run_deploy(); self.assertEqual(result.returncode,0,result.stderr)
        compose_calls=[args for args in self.calls() if args[:2]==['docker','compose']]
        self.assertTrue(compose_calls)
        self.assertTrue(all('compose.authorization.yml' in args for args in compose_calls))
    def test_authorization_overlay_missing_key_stops_before_docker(self):
        self.env['UPDATE_INSTALL_AUTHORIZATION']='1'
        result=self.run_deploy(); self.assertNotEqual(result.returncode,0)
        self.assertIn('Missing install authorization key',result.stderr)
        self.assertFalse((self.root/'calls.jsonl').exists())
        self.assertEqual(self.state()['active'],'blue')
    def test_authorization_overlay_setting_persists_in_deployment_env(self):
        (self.root/'secrets').mkdir()
        (self.root/'secrets/install-authorization.key').write_text('a'*64)
        (self.root/'deploy.env').write_text('UPDATE_INSTALL_AUTHORIZATION=1\nUPDATE_AUTHORIZATION_KEY_ID=test\n')
        self.env.pop('UPDATE_INSTALL_AUTHORIZATION',None)
        result=self.run_deploy(); self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(all('compose.authorization.yml' in args for args in self.calls() if args[:2]==['docker','compose']))
    def test_migration_and_pre_switch_smoke_failure_preserve_live_service(self):
        for scenario in ('migration-fail','smoke-before','proxy-fail'):
            self.control(scenario)
            result=self.run_deploy(); self.assertNotEqual(result.returncode,0)
            self.assertEqual(self.state()['active'],'blue'); self.assertEqual(self.state()['stopped'],[])
            self.assertIn('blue',(self.root/'state/routing/active.map').read_text())
    def test_post_switch_failures_return_to_old_slot(self):
        for scenario in ('smoke-after','health-after'):
            self.control(scenario)
            result=self.run_deploy(); self.assertNotEqual(result.returncode,0)
            self.assertEqual(self.state()['active'],'blue'); self.assertEqual(self.state()['stopped'],[])
            self.assertEqual(json.loads((self.root/'state/deployment.json').read_text())['phase'],'rolled_back')
    def test_drain_timeout_retains_old_slot(self):
        self.control('drain-busy'); self.env['UPDATE_DRAIN_SECONDS']='0'
        result=self.run_deploy(); self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.state()['active'],'green'); self.assertEqual(self.state()['stopped'],[])
        self.assertEqual(json.loads((self.root/'state/deployment.json').read_text())['phase'],'cleanup_deferred')
    def test_recovery_uses_live_route_instead_of_stale_disk_record(self):
        self.control('success',active='green')
        result=self.run_deploy(); self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.state()['active'],'blue'); self.assertEqual(self.state()['stopped'],['update-server-green'])
    def test_rollback_does_not_downgrade_or_migrate_database(self):
        result=self.run_deploy(mode='rollback'); self.assertEqual(result.returncode,0,result.stderr)
        self.assertFalse(any('run' in args and 'migrate' in args for args in self.calls()))
    def test_moving_tag_is_rejected(self):
        self.assertNotEqual(self.run_deploy(image='ghcr.io/example/updates:latest').returncode,0)
        self.assertEqual(self.state()['active'],'blue')
    def test_bootstrap_does_not_replace_existing_proxy(self):
        self.assertNotEqual(self.run_deploy(mode='bootstrap').returncode,0)
        self.assertEqual(self.state()['stopped'],[])
    def test_deployment_lock_blocks_concurrent_commands(self):
        import fcntl
        lock=self.root/'state/deploy.lock'
        with lock.open('w') as handle:
            fcntl.flock(handle,fcntl.LOCK_EX|fcntl.LOCK_NB)
            self.assertNotEqual(self.run_deploy().returncode,0)
            self.assertEqual(self.state()['stopped'],[])

if __name__=='__main__': unittest.main(verbosity=2)
