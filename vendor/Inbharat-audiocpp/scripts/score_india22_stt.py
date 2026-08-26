#!/usr/bin/env python3
"""Score per-language STT WER/CER for InBharat's India-22 benchmark.

Input JSONL rows: {"id", "language", "reference", "hypothesis"}. This tool computes
plain, reproducible edit-distance metrics; it does not hide missing languages or average
them away. Use --require-all-22 for a release gate.
"""
from __future__ import annotations
import argparse, json, sys
from collections import defaultdict
from pathlib import Path

LANGUAGES = ["as-IN","bn-IN","brx-IN","doi-IN","gu-IN","hi-IN","kn-IN","ks-IN","kok-IN","mai-IN","ml-IN","mni-IN","mr-IN","ne-IN","or-IN","pa-IN","sa-IN","sat-IN","sd-IN","ta-IN","te-IN","ur-IN"]

def edit_distance(a, b):
    previous = list(range(len(b)+1))
    for i, x in enumerate(a, 1):
        current = [i]
        for j, y in enumerate(b, 1):
            current.append(min(current[-1]+1, previous[j]+1, previous[j-1]+(x != y)))
        previous = current
    return previous[-1]

def normalize(text):
    return " ".join(str(text).casefold().split())

def main():
    p=argparse.ArgumentParser()
    p.add_argument("input", type=Path)
    p.add_argument("--require-all-22", action="store_true")
    p.add_argument("--output", type=Path)
    args=p.parse_args()
    totals=defaultdict(lambda:{"word_errors":0,"words":0,"char_errors":0,"chars":0,"cases":0})
    ids=set()
    for number,line in enumerate(args.input.read_text(encoding="utf-8").splitlines(),1):
        if not line.strip(): continue
        row=json.loads(line)
        for key in ("id","language","reference","hypothesis"):
            if key not in row: raise SystemExit(f"line {number}: missing {key}")
        if row["id"] in ids: raise SystemExit(f"line {number}: duplicate id {row['id']}")
        ids.add(row["id"])
        lang=row["language"]
        if lang not in LANGUAGES: raise SystemExit(f"line {number}: unknown language {lang}")
        ref=normalize(row["reference"]); hyp=normalize(row["hypothesis"])
        rw=ref.split(); hw=hyp.split()
        t=totals[lang]
        t["word_errors"]+=edit_distance(rw,hw); t["words"]+=len(rw)
        t["char_errors"]+=edit_distance(list(ref),list(hyp)); t["chars"]+=len(ref)
        t["cases"]+=1
    missing=[x for x in LANGUAGES if totals[x]["cases"]==0]
    if args.require_all_22 and missing: raise SystemExit("missing languages: "+",".join(missing))
    report={"schema":"inbharat.india22.stt-report.v1","case_count":len(ids),"missing_languages":missing,"languages":{}}
    for lang in LANGUAGES:
        t=totals[lang]
        if not t["cases"]: continue
        report["languages"][lang]={**t,"wer":t["word_errors"]/max(1,t["words"]),"cer":t["char_errors"]/max(1,t["chars"])}
    rendered=json.dumps(report,ensure_ascii=False,indent=2)+"\n"
    if args.output: args.output.write_text(rendered,encoding="utf-8")
    else: sys.stdout.write(rendered)
if __name__=="__main__": main()
