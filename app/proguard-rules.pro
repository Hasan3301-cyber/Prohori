# uniffi's generated bindings are reached through JNA direct mapping, which resolves
# native symbols by reflection. R8 has no way to see those uses, so the classes have to
# survive shrinking or the app dies on the first FFI call — in release only, which is the
# build a user has.
-keep class org.prohori.core.** { *; }
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { *; }
-dontwarn java.awt.**
-dontwarn java.lang.ref.Cleaner*
