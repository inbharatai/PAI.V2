#!/usr/bin/env python3
"""Scoped Kotlin policy tests and Android API/Compose compile, not a Gradle/device gate.
Uses existing compiler/cache plus exactly recorded official Google SDK jars. No fake SDK.
"""
import argparse,json,subprocess,hashlib
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--api',action='store_true');p.add_argument('--evidence',type=Path,required=True);p.add_argument('--sdk-jars',type=Path,required=True);a=p.parse_args()
root=Path(__file__).resolve().parents[3];android=root/'android-app/UnoOneAgent';g=Path('/home/vercel-sandbox/.gradle/caches');jars=root/'target/personal-kotlin-jars';ev=a.evidence;ev.mkdir(parents=True,exist_ok=True)
compiler=':'.join(str(x) for x in jars.glob('*.jar'));cp=list(jars.glob('*.jar'));src=[android/'app/src/main/java/com/unoone/agent/providers/ProviderContracts.kt'];plugins=[jars/'kotlin-serialization-compiler-plugin-embeddable-2.2.21.jar']
if a.api:
 cp+=[Path('/agent/workspace/toolchains/android-sdk/platforms/android-35/android.jar')]+list(a.sdk_jars.glob('*.jar'))+list(g.glob('*/transforms/**/jars/classes.jar'))
 for group in ['org.jetbrains.kotlinx','androidx.annotation','androidx.collection','androidx.lifecycle','androidx.compose.runtime','androidx.compose.ui','org.bouncycastle']:
  cp+=list((g/'modules-2/files-2.1'/group).glob('**/*.jar'))
 cp+=list(android.glob('*/build/tmp/kotlin-classes/debug'))+list(android.glob('*/build/intermediates/**/R.jar'))+list(android.glob('*/build/intermediates/javac/debug/compileDebugJavaWithJavac/classes'))
 cp=[x for x in cp if not ('lifecycle' in str(x) and any(v in str(x) for v in ['2.5.1','2.6.2','2.4.1','2.1.0'])) and 'activity-1.6.0' not in str(x)]
 src=list((android/'app/src/main/java/com/unoone/agent/providers').glob('*.kt'))+list((android/'app/src/main/java/com/unoone/agent/personal').glob('*.kt'))
 src+=list((android/'core/src/main/java/com/unoone/agent/core/personal').glob('*.kt'))
 src+=[android/'app/src/main/java/com/unoone/agent/vaultbridge/VaultConnection.kt']+[android/'vault/src/main/java/com/unoone/agent/vault'/f'{s}.kt' for s in ['MobileVaultRepository','VaultCrypto','NativeVaultKdf','PrivateFileVaultIO','VaultRecordReader','VaultRecordWriter','SafVaultIO']]
 plugins+=list(g.glob('modules-2/files-2.1/org.jetbrains.kotlin/kotlin-compose-compiler-plugin-embeddable/2.2.21/**/*.jar'))[:1]
else:src+=list((root/'packages/personal-provider-adapters/tests/kotlin').glob('*.kt'))
cp=':'.join(dict.fromkeys(str(x) for x in cp));name='android-api' if a.api else 'kotlin-policy';output=ev/(name+'.jar')
cmd=['java','-Xmx900m','-cp',compiler,'org.jetbrains.kotlin.cli.jvm.K2JVMCompiler','-no-stdlib','-no-reflect','-jvm-target','17','-classpath',cp]+['-Xplugin='+str(x) for x in plugins]+['-d',str(output)]+[str(x) for x in src]
def run(command,label):
 r=subprocess.run(command,cwd=root,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True);(ev/(label+'.log')).write_text(r.stdout);print(r.stdout)
 (ev/(label+'.json')).write_text(json.dumps({'command':command,'exit':r.returncode,'source_sha256':{str(x):hashlib.sha256(x.read_bytes()).hexdigest() for x in src}},indent=2))
 if r.returncode:raise SystemExit(r.returncode)
run(cmd,name+'-compile')
if not a.api:run(['java','-Xmx600m','-cp',str(output)+':'+cp,'org.junit.runner.JUnitCore','com.unoone.agent.providers.ProviderPolicyTest'],name+'-tests')
