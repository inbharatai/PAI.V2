#!/usr/bin/env python3
"""Run ONLY the personal contract JVM tests with an existing Kotlin compiler cache.
No Gradle/Android build, source copying or dependency download is performed.
Usage: python3 tools/test_kotlin.py --jars /path/to/compiler-and-runtime-jars
Compiler/plugin 2.2.21, serialization-json/core-jvm 1.8.0, JUnit 4.13.2,
Hamcrest 1.3 and compiler runtime dependencies must be present in --jars.
These match the Android build's existing compiler/serialization/test versions.
"""
import argparse
from pathlib import Path
import os
import subprocess
p = argparse.ArgumentParser()
p.add_argument('--jars', type=Path, required=True)
a = p.parse_args()
root = Path(__file__).resolve().parents[3]
package = root / 'packages/personal-agent-contracts'
cp = os.pathsep.join(str(f) for f in sorted(a.jars.glob('*.jar')))
plugin = a.jars / 'kotlin-serialization-compiler-plugin-embeddable-2.2.21.jar'
assert plugin.is_file(), 'Kotlin 2.2.21 serialization compiler plugin is required'
output = root / 'target/personal-kotlin-tests'
output.mkdir(parents=True, exist_ok=True)
src = root / 'android-app/UnoOneAgent/core/src'
sources = sorted((src/'main/java/com/unoone/agent/core/personal').glob('*.kt')) + sorted((src/'test/java/com/unoone/agent/core/personal').glob('*.kt'))
subprocess.run(['java', '-Xmx768m', '-cp', cp, 'org.jetbrains.kotlin.cli.jvm.K2JVMCompiler', '-no-stdlib', '-no-reflect', '-jvm-target', '17', '-classpath', cp, '-Xplugin='+str(plugin), '-d', str(output)] + [str(s) for s in sources], check=True)
subprocess.run(['java', '-Xmx512m', '-Dpersonal.contract.fixtures='+str(package/'fixtures'), '-cp', str(output)+os.pathsep+cp, 'org.junit.runner.JUnitCore', 'com.unoone.agent.core.personal.PersonalContractConformanceTest'], check=True)
