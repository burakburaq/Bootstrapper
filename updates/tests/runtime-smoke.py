"""HTTP smoke for the isolated container with intentionally unconfigured R2."""
import json
import sys
import urllib.error
import urllib.request
origin=sys.argv[1].rstrip('/')
def request(path,expected,body=None):
    data=json.dumps(body).encode() if body is not None else None
    req=urllib.request.Request(origin+path,data=data,headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=30) as response: status=response.status; payload=response.read()
    except urllib.error.HTTPError as error: status=error.code; payload=error.read()
    assert status==expected,(path,status,expected,payload)
    return json.loads(payload) if payload else None
assert request('/health/live',200)['instance']=='container-test'
for app in ('alpha','beta'):
    request(f'/v1/tauri/{app}/production/windows-x86_64/1.0.0',503)
    request('/v1/update/check',503,dict(app_id=app,channel='production',target='windows-x86_64',current_version='1.0.0',installation_id='runtime-smoke',supports_delta=False,base_sha256=None,base_size_bytes=None,schema_version=1))
    request(f'/v1/tauri/{app}/staging/windows-x86_64/1.0.0',401)
    request(f'/v1/admin/apps/{app}/channels/production/releases/unknown?target=windows-x86_64',401)
request('/v1/tauri/unknown/production/windows-x86_64/1.0.0',404)
request('/health/ready',503)
print('Container HTTP smoke passed: two applications, auth, DB, unavailable R2 -> 503')
