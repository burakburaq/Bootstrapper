"""Emit public-only test configuration for container startup tests; never production keys."""
import base64
import json
import sys
keys=json.load(open(sys.argv[1],encoding='utf-8-sig'))
public=base64.b64decode(next(iter(keys.values())))
package=base64.b64encode(b'Ed'+bytes([1])*8+public).decode()
apps=[]
for app in ('alpha','beta'):
    apps.append(dict(app_id=app,channels={'production':{'private':False,'read_token_sha256':None},'staging':{'private':True,'read_token_sha256':'0'*64}},
        targets=['windows-x86_64'],metadata_keys=keys,package_public_key=package,bucket=f'{app}-updates',health_object_key=f'{app}/health.txt',admin_tokens=[]))
json.dump(dict(public_url='https://updates.example.com/',r2_endpoint='https://unconfigured.r2.cloudflarestorage.com/',apps=apps,max_artifact_bytes=1024*1024,presign_ttl_seconds=300),sys.stdout,indent=2)
