# uniffi's generated bindings are reached through JNA direct mapping, which resolves
# native symbols by reflection. R8 has no way to see those uses, so the classes have to
# survive shrinking or the app dies on the first FFI call — in release only, which is the
# build a user has.
-keep class org.prohori.core.** { *; }
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }
-dontwarn java.awt.**
-dontwarn java.lang.ref.Cleaner*

# The streaming callback is called from C++ by name, through GetMethodID("onToken"). The
# default Android rules keep native method names, so `generateNative` survives shrinking,
# but nothing tells R8 that this ordinary Kotlin interface method is reached the same way.
# Renamed, the native side finds no method, and release builds lose live text while debug
# builds keep it — the hardest kind of difference to notice.
-keep interface org.prohori.app.TokenSink { *; }
-keepclassmembers class * implements org.prohori.app.TokenSink { *; }
