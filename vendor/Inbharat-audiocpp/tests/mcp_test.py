#!/usr/bin/env python3
import json,subprocess,sys
from pathlib import Path
binary=Path(sys.argv[1]); root=Path(sys.argv[2])
requests=[
 {"jsonrpc":"2.0","id":1,"method":"server/discover"},
 {"jsonrpc":"2.0","id":2,"method":"tools/list"},
 {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"audio.language_packs","arguments":{"root":str(root/'packs')}}},
]
proc=subprocess.run([str(binary)],input='\n'.join(json.dumps(x) for x in requests)+'\n',text=True,capture_output=True,check=True)
responses=[json.loads(x) for x in proc.stdout.splitlines() if x.strip()]
assert responses[0]['result']['protocolVersion']=='2026-07-28'
tools=[x['name'] for x in responses[1]['result']['tools']]
assert 'audio.language_packs' in tools and 'audio.detect_language' in tools
catalog=json.loads(responses[2]['result']['content'][0]['text'])
assert catalog['schema']=='inbharat.language-pack-catalog.v1'
assert len(catalog['packs'])==22
assert {x['language'] for x in catalog['packs']}=={'as-IN','bn-IN','brx-IN','doi-IN','gu-IN','hi-IN','kn-IN','ks-IN','kok-IN','mai-IN','ml-IN','mni-IN','mr-IN','ne-IN','or-IN','pa-IN','sa-IN','sat-IN','sd-IN','ta-IN','te-IN','ur-IN'}
print('PASS mcp-control-plane')
