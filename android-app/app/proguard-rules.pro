# JukePI release shrinking rules. The libraries (kotlinx.serialization, OkHttp, Media3, WorkManager, Coil) ship their
# own consumer rules; these cover what is specific to this app.

# JSON models decoded by name through kotlinx.serialization.
-keep,includedescriptorclasses class app.musicremote.**$$serializer { *; }
-keepclassmembers class app.musicremote.** { *** Companion; }
-keepclasseswithmembers class app.musicremote.** { kotlinx.serialization.KSerializer serializer(...); }

# Optional OkHttp TLS providers that are not on the class path.
-dontwarn org.bouncycastle.**
-dontwarn org.conscrypt.**
-dontwarn org.openjsse.**
