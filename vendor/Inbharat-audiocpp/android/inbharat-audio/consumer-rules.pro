# JNI entrypoints are named methods on this class.
-keep class org.inbharat.audio.NativeBridge { *; }
-keepclasseswithmembernames class * { native <methods>; }
