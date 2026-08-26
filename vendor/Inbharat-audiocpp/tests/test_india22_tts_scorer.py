#!/usr/bin/env python3
import json,subprocess,sys,tempfile
from pathlib import Path
root=Path(__file__).resolve().parents[1]; scorer=root/'scripts'/'score_india22_tts.py'
rows=[{"id":"ta-1","language":"ta-IN","reference_text":"வணக்கம்","independent_asr_transcript":"வணக்கம்","ttfa_ms":180,"rtf":0.25,"native_mos_ratings":[4,5]},{"id":"hi-1","language":"hi-IN","reference_text":"नमस्ते दुनिया","independent_asr_transcript":"नमस्ते","ttfa_ms":200,"rtf":0.3}]
with tempfile.TemporaryDirectory() as td:
 p=Path(td)/'rows.jsonl';p.write_text('\n'.join(json.dumps(x,ensure_ascii=False) for x in rows)+'\n',encoding='utf-8')
 r=json.loads(subprocess.check_output([sys.executable,str(scorer),str(p)],text=True))
 assert r['languages']['ta-IN']['intelligibility_cer']==0.0
 assert r['languages']['ta-IN']['native_mos']==4.5
 assert r['languages']['hi-IN']['native_mos'] is None
 failed=subprocess.run([sys.executable,str(scorer),str(p),'--require-all-22'],capture_output=True,text=True)
 assert failed.returncode!=0 and 'missing languages' in failed.stderr
print('PASS india22-tts-scorer')
