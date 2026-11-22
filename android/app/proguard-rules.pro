# Keep kotlinx.serialization generated serializers and models
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.AnnotationsKt

-keepclassmembers class kotlinx.serialization.json.** {
    *** Companion;
}
-keepclasseswithmembers class kotlinx.serialization.json.** {
    kotlinx.serialization.KSerializer serializer(...);
}

-keep,includedescriptorclasses class org.akhterm.mobile.data.model.**$$serializer { *; }
-keepclassmembers class org.akhterm.mobile.data.model.** {
    *** Companion;
}
-keepclasseswithmembers class org.akhterm.mobile.data.model.** {
    kotlinx.serialization.KSerializer serializer(...);
}
