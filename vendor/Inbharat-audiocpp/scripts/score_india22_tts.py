#!/usr/bin/env python3
"""Score TTS intelligibility/latency per Indian language without inventing MOS.

Input JSONL: id, language, reference_text, independent_asr_transcript, ttfa_ms, rtf,
optional native_mos_ratings (list of 1..5). Missing human ratings remain null.
"""
from __future__ import annotations
import argparse,json,statistics,sys
from collections import defaultdict
from pathlib import Path
from score_india22_stt import LANGUAGES, edit_distance, normalize

def main():
 p=argparse.ArgumentParser();p.add_argument('input',type=Path);p.add_argument('--require-all-22',action='store_true');p.add_argument('--output',type=Path);a=p.parse_args()
 totals=defaultdict(lambda:{'char_errors':0,'chars':0,'cases':0,'ttfa_ms':[],'rtf':[],'mos':[]})
 ids=set()
 for n,line in enumerate(a.input.read_text(encoding='utf-8').splitlines(),1):
  if not line.strip():continue
  row=json.loads(line)
  for k in ('id','language','reference_text','independent_asr_transcript','ttfa_ms','rtf'):
   if k not in row:raise SystemExit(f'line {n}: missing {k}')
  if row['id'] in ids:raise SystemExit(f"line {n}: duplicate id {row['id']}")
  ids.add(row['id']);lang=row['language']
  if lang not in LANGUAGES:raise SystemExit(f'line {n}: unknown language {lang}')
  ref=normalize(row['reference_text']);hyp=normalize(row['independent_asr_transcript']);t=totals[lang]
  t['char_errors']+=edit_distance(list(ref),list(hyp));t['chars']+=len(ref);t['cases']+=1
  ttfa=float(row['ttfa_ms']);rtf=float(row['rtf'])
  if ttfa<0 or rtf<0:raise SystemExit(f'line {n}: negative latency')
  t['ttfa_ms'].append(ttfa);t['rtf'].append(rtf)
  for rating in row.get('native_mos_ratings',[]):
   r=float(rating)
   if r<1 or r>5:raise SystemExit(f'line {n}: MOS rating outside 1..5')
   t['mos'].append(r)
 missing=[x for x in LANGUAGES if totals[x]['cases']==0]
 if a.require_all_22 and missing:raise SystemExit('missing languages: '+','.join(missing))
 report={'schema':'inbharat.india22.tts-report.v1','case_count':len(ids),'missing_languages':missing,'languages':{}}
 for lang in LANGUAGES:
  t=totals[lang]
  if not t['cases']:continue
  report['languages'][lang]={'cases':t['cases'],'intelligibility_cer':t['char_errors']/max(1,t['chars']),'mean_ttfa_ms':statistics.fmean(t['ttfa_ms']),'mean_rtf':statistics.fmean(t['rtf']),'native_mos':statistics.fmean(t['mos']) if t['mos'] else None,'native_mos_rating_count':len(t['mos'])}
 rendered=json.dumps(report,ensure_ascii=False,indent=2)+'\n'
 if a.output:a.output.write_text(rendered,encoding='utf-8')
 else:sys.stdout.write(rendered)
if __name__=='__main__':main()
