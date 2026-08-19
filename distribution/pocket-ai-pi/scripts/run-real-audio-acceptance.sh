#!/usr/bin/env bash
set -euo pipefail
ROOT=${1:?usage: run-real-audio-acceptance.sh ROOT ASR_FIXTURE TTS_TEXT [EXPECTED] [LANGUAGE]}
FIXTURE=${2:?missing ASR fixture}; TTS_TEXT=${3:?missing TTS text}; EXPECTED=${4:-}; LANG=${5:-en}
ROOT=$(realpath "$ROOT"); FIXTURE=$(realpath "$FIXTURE")
CFG="$ROOT/SPEECH/config/inbharat-audio.v1.json"; [ -s "$CFG" ] || { echo 'FAIL: speech config missing' >&2; exit 2; }
eval "$(python3 - "$CFG" <<'PY'
import json,shlex,sys
m=json.load(open(sys.argv[1],encoding='utf-8-sig'))
if not m.get('enabled'):raise SystemExit('speech config disabled')
for k,v in [('UPSTREAM',m['upstream_commit']),('BACKEND',m['backend']),('ASR_FAMILY',m['asr']['family']),('ASR_REL',m['asr']['model_relative_path']),('TTS_FAMILY',m['tts']['family']),('TTS_REL',m['tts']['model_relative_path'])]:print(f'{k}={shlex.quote(str(v))}')
PY
)"
IBAUDIO="$ROOT/RUNTIMES/LINUX-ARM64/AUDIO/ibaudio"; AUDIOCPP="$ROOT/RUNTIMES/LINUX-ARM64/AUDIOCPP/audiocpp_cli"
for x in "$IBAUDIO" "$AUDIOCPP" "$FIXTURE"; do [ -f "$x" ] || { echo "FAIL: missing $x" >&2; exit 3; }; done
STATUS=$($IBAUDIO audio-cpp-status --json); export STATUS UPSTREAM
python3 - <<'PY'
import json,os
s=json.loads(os.environ['STATUS'])
if not s.get('adapter_compiled'):raise SystemExit('InBharat Audio not built against reviewed audio.cpp source')
if s.get('reviewed_commit','').lower()!=os.environ['UPSTREAM'].lower():raise SystemExit('reviewed commit mismatch')
PY
ASR="$ROOT/$ASR_REL";TTS="$ROOT/$TTS_REL"; [ -e "$ASR" ] && [ -e "$TTS" ] || { echo 'FAIL: speech models missing' >&2; exit 4; }
OUT="$ROOT/SPEECH/acceptance/artifacts";mkdir -p "$OUT";TRANS="$OUT/asr-transcript.txt";WAV="$OUT/tts-output.wav";rm -f "$TRANS" "$WAV"
"$AUDIOCPP" --task asr --family "$ASR_FAMILY" --model "$ASR" --backend "$BACKEND" --audio "$FIXTURE" --language "$LANG" --text-out "$TRANS"
[ -s "$TRANS" ] || { echo 'FAIL: real ASR transcript empty' >&2; exit 5; }
if [ -n "$EXPECTED" ]; then grep -Fqi "$EXPECTED" "$TRANS" || { echo "FAIL: transcript semantic check failed: $(cat "$TRANS")" >&2; exit 5; }; fi
"$AUDIOCPP" --task tts --family "$TTS_FAMILY" --model "$TTS" --backend "$BACKEND" --text "$TTS_TEXT" --language "$LANG" --out "$WAV"
python3 - "$WAV" <<'PY'
import struct,sys,wave
p=sys.argv[1]
with open(p,'rb') as f:
    if f.read(4)!=b'RIFF':raise SystemExit('TTS not RIFF')
    f.seek(8)
    if f.read(4)!=b'WAVE':raise SystemExit('TTS not WAVE')
with wave.open(p,'rb') as w:
    if w.getframerate()<=0 or w.getnchannels()<=0 or w.getnframes()<=0:raise SystemExit('invalid TTS WAV')
PY
python3 - "$ROOT" "$FIXTURE" "$TRANS" "$WAV" "$LANG" <<'PY'
import hashlib,json,os,sys,wave,datetime
from pathlib import Path
root,fixture,trans,wav,lang=(Path(sys.argv[1]),Path(sys.argv[2]),Path(sys.argv[3]),Path(sys.argv[4]),sys.argv[5])
def fsha(p):
 h=hashlib.sha256();
 with open(p,'rb') as f:
  for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
 return h.hexdigest()
def tsha(p):
 if p.is_file():return fsha(p)
 h=hashlib.sha256();h.update(b'IBAUDIO_TREE_SHA256_V1\n')
 for x in sorted((x for x in p.rglob('*') if x.is_file()),key=lambda x:x.relative_to(p).as_posix()):
  rel=x.relative_to(p).as_posix();h.update(rel.encode());h.update(b'\0');h.update(fsha(x).encode());h.update(b'\n')
 return h.hexdigest()
cfg=json.load(open(root/'SPEECH/config/inbharat-audio.v1.json',encoding='utf-8-sig'))
with wave.open(str(wav),'rb') as w: rate=w.getframerate();duration=w.getnframes()/rate
acc={'schema':'inbharat.pai.audio_cpp_acceptance.v1','upstream_commit':cfg['upstream_commit'],'platform':'LINUX-ARM64','backend':cfg['backend'],'audiocpp_cli_sha256':fsha(root/'RUNTIMES/LINUX-ARM64/AUDIOCPP/audiocpp_cli'),'ibaudio_cli_sha256':fsha(root/'RUNTIMES/LINUX-ARM64/AUDIO/ibaudio'),'asr':{'family':cfg['asr']['family'],'model_relative_path':cfg['asr']['model_relative_path'],'model_sha256':tsha(root/cfg['asr']['model_relative_path']),'language':lang,'fixture_sha256':fsha(fixture),'transcript_sha256':fsha(trans),'transcript_bytes':trans.stat().st_size},'tts':{'family':cfg['tts']['family'],'model_relative_path':cfg['tts']['model_relative_path'],'model_sha256':tsha(root/cfg['tts']['model_relative_path']),'language':lang,'output_sha256':fsha(wav),'sample_rate':rate,'duration_seconds':duration},'tested_at':datetime.datetime.now(datetime.timezone.utc).isoformat()}
out=root/'SPEECH/acceptance/audio-cpp.acceptance.v1.json';out.parent.mkdir(parents=True,exist_ok=True);out.write_text(json.dumps(acc,indent=2)+'\n','utf-8');print('PASS:',out)
PY
