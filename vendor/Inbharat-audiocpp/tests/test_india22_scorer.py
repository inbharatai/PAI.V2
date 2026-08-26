#!/usr/bin/env python3
import json, subprocess, sys, tempfile
from pathlib import Path

root=Path(__file__).resolve().parents[1]
scorer=root/'scripts'/'score_india22_stt.py'
rows=[
 {"id":"hi-1","language":"hi-IN","reference":"नमस्ते दुनिया","hypothesis":"नमस्ते दुनिया"},
 {"id":"as-1","language":"as-IN","reference":"মই অসমীয়া কওঁ","hypothesis":"মই অসমীয়া"},
]
with tempfile.TemporaryDirectory() as td:
 p=Path(td)/'rows.jsonl'; p.write_text('\n'.join(json.dumps(x,ensure_ascii=False) for x in rows)+'\n',encoding='utf-8')
 out=subprocess.check_output([sys.executable,str(scorer),str(p)],text=True)
 report=json.loads(out)
 assert report['languages']['hi-IN']['wer']==0.0
 assert report['languages']['as-IN']['word_errors']==1
 assert 'bn-IN' in report['missing_languages']
 failed=subprocess.run([sys.executable,str(scorer),str(p),'--require-all-22'],capture_output=True,text=True)
 assert failed.returncode!=0 and 'missing languages' in failed.stderr
print('PASS india22-stt-scorer')
