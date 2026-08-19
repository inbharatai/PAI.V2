#!/usr/bin/env python3
from __future__ import annotations
import argparse,json,urllib.request,urllib.error,sys,time
ap=argparse.ArgumentParser();ap.add_argument('--port',type=int,required=True);ap.add_argument('--expected-model',required=True);ap.add_argument('--timeout',type=float,default=180);a=ap.parse_args()
end=time.time()+a.timeout;last='not started'
while time.time()<end:
    try:
        with urllib.request.urlopen(f'http://127.0.0.1:{a.port}/health',timeout=2) as r: health=json.load(r)
        with urllib.request.urlopen(f'http://127.0.0.1:{a.port}/v1/models',timeout=2) as r: models=json.load(r)
        ids=[str(x.get('id','')) for x in models.get('data',[])]
        if a.expected_model in ids:
            print(json.dumps({'status':'ready','model_id':a.expected_model,'health':health},sort_keys=True));sys.exit(0)
        last=f'model id not present: {ids}'
    except Exception as e:last=str(e)
    time.sleep(.5)
print(f'FAIL: model server did not become verified-ready: {last}',file=sys.stderr);sys.exit(1)
