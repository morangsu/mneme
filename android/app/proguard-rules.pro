# ---- uniffi / JNI ----
# uniffi 生成的 Kotlin 绑定通过 JNI 调用 Rust，类名与方法名不能被混淆或裁剪。
# 绑定接入后位于 io.github.morangsu.mneme.ffi 包。
-keep class io.github.morangsu.mneme.ffi.** { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
# 反射调用到的 Kotlin 数据类保持字段名
-keepclassmembers class io.github.morangsu.mneme.** { *; }

# ---- org.json ----
-keep class org.json.** { *; }

# ---- WorkManager / ML Kit ----
# 二者自带 consumer rules；这里仅保证 Worker 构造器可见。
-keep class io.github.morangsu.mneme.SyncWorker { *; }
-dontwarn com.google.mlkit.**
