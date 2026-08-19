#!/usr/bin/env python3
from __future__ import annotations
import argparse,datetime,hashlib,json,os,shutil,sys
from pathlib import Path

TREE_PREFIX=b"IBAUDIO_TREE_SHA256_V1\n"
def file_sha(p:Path)->str:
    h=hashlib.sha256()
    with p.open('rb') as f:
        for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
    return h.hexdigest()
def tree_sha(p:Path)->str:
    if p.is_file(): return file_sha(p)
    h=hashlib.sha256();h.update(TREE_PREFIX)
    files=[]
    for x in p.rglob('*'):
        if x.is_symlink(): raise ValueError(f'symlink refused in model package: {x}')
        if x.is_file():files.append(x)
    if not files: raise ValueError(f'empty model package: {p}')
    for x in sorted(files,key=lambda z:z.relative_to(p).as_posix()):
        rel=x.relative_to(p).as_posix();h.update(rel.encode());h.update(b'\0');h.update(file_sha(x).encode());h.update(b'\n')
    return h.hexdigest()
def copy_file(src:Path,dst:Path,exe=False):
    if not src.is_file():raise FileNotFoundError(src)
    if src.is_symlink():raise ValueError(f'symlink refused: {src}')
    dst.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(src,dst)
    if exe:dst.chmod(dst.stat().st_mode|0o111)
def copy_tree(src:Path,dst:Path):
    if not src.exists():raise FileNotFoundError(src)
    if src.is_symlink():raise ValueError(f'symlink refused: {src}')
    if src.is_file():copy_file(src,dst);return
    for x in src.rglob('*'):
        if x.is_symlink():raise ValueError(f'symlink refused in model package: {x}')
        if x.is_file():copy_file(x,dst/x.relative_to(src))
def add_tree_assets(root:Path,base:Path,assets:list):
    if base.is_file():
        rel=base.relative_to(root).as_posix();assets.append((rel,base,False));return
    for x in sorted((z for z in base.rglob('*') if z.is_file()),key=lambda z:z.relative_to(root).as_posix()):
        assets.append((x.relative_to(root).as_posix(),x,False))
def read_json(p:Path):return json.loads(p.read_text('utf-8-sig'))
def safe_rel(rel:str)->str:
    p=Path(rel)
    if p.is_absolute() or '..' in p.parts or not rel.strip():raise ValueError(f'unsafe relative path: {rel}')
    return p.as_posix()

def main():
    ap=argparse.ArgumentParser(description='Build a final Pocket AI Pi package from real, acceptance-tested artifacts only.')
    ap.add_argument('--out',required=True)
    ap.add_argument('--harness',required=True);ap.add_argument('--ibaudio',required=True);ap.add_argument('--audiocpp',required=True)
    ap.add_argument('--llama-cpu',required=True);ap.add_argument('--llama-vulkan')
    ap.add_argument('--model',required=True);ap.add_argument('--model-id',required=True);ap.add_argument('--model-rel',required=True)
    ap.add_argument('--speech-config',required=True);ap.add_argument('--audio-acceptance',required=True)
    ap.add_argument('--asr-model',required=True);ap.add_argument('--tts-model',required=True)
    a=ap.parse_args()
    out=Path(a.out).resolve();out.mkdir(parents=True,exist_ok=True)
    if any(out.iterdir()):raise ValueError(f'output must be empty: {out}')
    speech=read_json(Path(a.speech_config)); acceptance=read_json(Path(a.audio_acceptance))
    if not speech.get('enabled'):raise ValueError('speech config must be enabled only after real acceptance')
    if acceptance.get('schema')!='inbharat.pai.audio_cpp_acceptance.v1':raise ValueError('invalid audio acceptance schema')
    if str(acceptance.get('platform','')).upper() not in ('LINUX-ARM64','LINUX_ARM64'):raise ValueError('audio acceptance must come from Linux ARM64/Pi, not another platform')
    if str(acceptance.get('upstream_commit','')).lower()!=str(speech.get('upstream_commit','')).lower():raise ValueError('speech/acceptance upstream commit mismatch')
    asr_src=Path(a.asr_model).resolve();tts_src=Path(a.tts_model).resolve()
    if tree_sha(asr_src)!=str(acceptance['asr']['model_sha256']).lower():raise ValueError('ASR model tree hash does not match physical acceptance')
    if tree_sha(tts_src)!=str(acceptance['tts']['model_sha256']).lower():raise ValueError('TTS model tree hash does not match physical acceptance')
    asr_rel=safe_rel(str(speech['asr']['model_relative_path']));tts_rel=safe_rel(str(speech['tts']['model_relative_path']));model_rel=safe_rel(a.model_rel)
    assets=[]
    fixed=[(a.harness,'RUNTIMES/LINUX-ARM64/HARNESS/inbharat-harness',True),(a.ibaudio,'RUNTIMES/LINUX-ARM64/AUDIO/ibaudio',True),(a.audiocpp,'RUNTIMES/LINUX-ARM64/AUDIOCPP/audiocpp_cli',True),(a.llama_cpu,'RUNTIMES/LINUX-ARM64/LLAMA/CPU/llama-server',True),(a.speech_config,'SPEECH/config/inbharat-audio.v1.json',False),(a.audio_acceptance,'SPEECH/acceptance/audio-cpp.acceptance.v1.json',False),(a.model,model_rel,False)]
    if a.llama_vulkan:fixed.append((a.llama_vulkan,'RUNTIMES/LINUX-ARM64/LLAMA/VULKAN/llama-server',True))
    for src,rel,exe in fixed:
        d=out/rel;copy_file(Path(src).resolve(),d,exe);assets.append((rel,d,exe))
    asr_dst=out/asr_rel;tts_dst=out/tts_rel;copy_tree(asr_src,asr_dst);copy_tree(tts_src,tts_dst);add_tree_assets(out,asr_dst,assets);add_tree_assets(out,tts_dst,assets)
    runtime={'schema':'inbharat.pocket_ai_pi.runtime.v1','model_id':a.model_id,'model_relative_path':model_rel,'model_sha256':file_sha(out/model_rel),'default_backend':'CPU','bind_address':'127.0.0.1','network_default':'deny'}
    cfg=out/'CONFIG';cfg.mkdir(parents=True,exist_ok=True);rp=cfg/'pocket-ai-pi.runtime.v1.json';rp.write_text(json.dumps(runtime,indent=2)+'\n','utf-8');assets.append((rp.relative_to(out).as_posix(),rp,False))
    manifest={'schema':'inbharat.pocket_ai_pi.package.v1','product_id':'POCKET_AI_PI','generated_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'target':'linux-aarch64','assets':[]}
    seen=set()
    for rel,p,exe in sorted(assets,key=lambda x:x[0]):
        if rel in seen:continue
        seen.add(rel);manifest['assets'].append({'path':rel,'sha256':file_sha(p),'size_bytes':p.stat().st_size,'executable':exe})
    (cfg/'pocket-ai-pi.manifest.v1.json').write_text(json.dumps(manifest,indent=2)+'\n','utf-8')
    print(f'PASS: final Pocket AI Pi package contains {len(seen)} hash-bound real files at {out}')
if __name__=='__main__':
    try:main()
    except Exception as e:print(f'FAIL: {e}',file=sys.stderr);raise SystemExit(1)
