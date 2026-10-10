# Public USB-vault types are intentionally retained by normal Kotlin metadata.
# Native JNI symbol resolves this exact class/method name after R8.
-keep class com.unoone.agent.vault.NativeVaultKdf { *; }
-keep class com.unoone.agent.vault.VaultMemoryProvider { *; }
