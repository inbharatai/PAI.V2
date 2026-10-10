#!/usr/bin/env python3
"""Narrow existing-jar Android API compile and JVM protocol tests. No Gradle/device claims."""
import argparse, json, subprocess, hashlib
from pathlib import Path
p=argparse.ArgumentParser(); p.add_argument('--jars',type=Path,required=True); p.add_argument('--gradle-cache',type=Path,required=True); p.add_argument('--android-jar',type=Path,required=True); p.add_argument('--evidence',type=Path,required=True); p.add_argument('--api',action='store_true'); a=p.parse_args()
root=Path(__file__).resolve().parents[3]; android=root/'android-app/UnoOneAgent'; g=a.gradle_cache; evidence=a.evidence; evidence.mkdir(parents=True,exist_ok=True)
compiler=':'.join(str(x) for x in a.jars.glob('*.jar'))
cp=list(a.jars.glob('*.jar'))+[a.android_jar]
cp+=list(g.glob('modules-2/files-2.1/org.bouncycastle/bcprov-jdk18on/1.80/**/*.jar'))
src=list((android/'core/src/main/java/com/unoone/agent/core/personal').glob('*.kt'))
src+=[android/'vault/src/main/java/com/unoone/agent/vault'/f'{s}.kt' for s in ['MobileVaultRepository','VaultCrypto','NativeVaultKdf','PrivateFileVaultIO','VaultRecordReader','VaultRecordWriter']]
personal=android/'app/src/main/java/com/unoone/agent/personal'; src+=[personal/'PersonalLedger.kt',personal/'SharedLedger.kt',personal/'PersonalStore.kt']
peer=android/'app/src/main/java/com/unoone/agent/peersync'; src+=[peer/'PeerProtocol.kt',peer/'NativePeerHttps.kt']
plugins=[a.jars/'kotlin-serialization-compiler-plugin-embeddable-2.2.21.jar']
if a.api:
    cp+=list(g.glob('*/transforms/**/jars/classes.jar'))
    for group in ['org.jetbrains.kotlinx','androidx.annotation','androidx.collection','androidx.lifecycle','androidx.compose.runtime','androidx.compose.ui']:
        cp+=list((g/'modules-2/files-2.1'/group).glob('**/*.jar'))
    cp+=list(android.glob('*/build/tmp/kotlin-classes/debug'))+list(android.glob('*/build/intermediates/**/R.jar'))+list(android.glob('*/build/intermediates/javac/debug/compileDebugJavaWithJavac/classes'))
    cp=[x for x in cp if not ('lifecycle' in str(x) and any(v in str(x) for v in ['2.5.1','2.6.2','2.4.1','2.1.0'])) and 'activity-1.6.0' not in str(x)]
    src+=[personal/'PersonalAgentService.kt',personal/'PersonalAgentScreen.kt',peer/'PeerSyncService.kt',peer/'PeerSyncScreen.kt',android/'app/src/main/java/com/unoone/agent/ui/navigation/UnoOneNavHost.kt',android/'app/src/main/java/com/unoone/agent/vaultbridge/VaultConnection.kt',android/'vault/src/main/java/com/unoone/agent/vault/SafVaultIO.kt']
    plugins+=list(g.glob('modules-2/files-2.1/org.jetbrains.kotlin/kotlin-compose-compiler-plugin-embeddable/2.2.21/**/*.jar'))[:1]
else:
    src+=list((root/'packages/local-peer-sync/tests').glob('*.kt'))
cp=':'.join(dict.fromkeys(str(x) for x in cp)); name='android-api' if a.api else 'kotlin-host'; output=evidence/(name+'.jar')
cmd=['java','-Xmx900m','-cp',compiler,'org.jetbrains.kotlin.cli.jvm.K2JVMCompiler','-no-stdlib','-no-reflect','-jvm-target','17','-classpath',cp]+['-Xplugin='+str(x) for x in plugins]+['-d',str(output)]+[str(x) for x in src]
def run(command, label):
    r=subprocess.run(command,cwd=root,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    (evidence/(label+'.log')).write_text(r.stdout); print(r.stdout)
    (evidence/(label+'.json')).write_text(json.dumps({'command':command,'exit':r.returncode,'source_sha256':{str(x):hashlib.sha256(x.read_bytes()).hexdigest() for x in src}},indent=2))
    if r.returncode: raise SystemExit(r.returncode)
run(cmd,name+'-compile')
if not a.api: run(['java','-Xmx512m','-Dpeer.interop='+str(evidence),'-cp',str(output)+':'+cp,'org.junit.runner.JUnitCore','com.unoone.agent.peersync.PeerProtocolTest','com.unoone.agent.peersync.SharedLedgerTest'],name+'-tests')
