#!/usr/bin/env python3
import hashlib,shutil,subprocess,sys,tempfile
from pathlib import Path
root=Path(__file__).resolve().parents[1]
validator=root/'scripts'/'validate_22_language_packs.py'
with tempfile.TemporaryDirectory() as td:
 sandbox=Path(td)/'repo';shutil.copytree(root,sandbox,ignore=shutil.ignore_patterns('build','.git'))
 packs=sandbox/'packs';outside=sandbox/'outside.json';outside.write_text('{}\n',encoding='utf-8')
 digest=hashlib.sha256(outside.read_bytes()).hexdigest()
 catalog=packs/'catalog.v1.tsv'
 lines=catalog.read_text(encoding='utf-8').splitlines()
 # Replace one legitimate path with a hash-valid traversal outside packs/.
 for i,line in enumerate(lines):
  if line.startswith('hi-IN\t'):
   fields=line.split('\t');fields[1]='../outside.json';fields[2]=digest;lines[i]='\t'.join(fields);break
 catalog.write_text('\n'.join(lines)+'\n',encoding='utf-8')
 result=subprocess.run([sys.executable,str(sandbox/'scripts'/'validate_22_language_packs.py')],capture_output=True,text=True)
 assert result.returncode!=0
 assert 'escapes pack root' in result.stdout
print('PASS language-pack-validator-security')
