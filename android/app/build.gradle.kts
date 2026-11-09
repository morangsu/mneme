plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "io.github.morangsu.mneme"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.morangsu.mneme"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        // 只保留 arm64：Rust 侧目前只交叉编译了 aarch64-linux-android，
        // 若不限制，JNA/ML Kit 会把 armeabi-v7a、x86 等空壳一起打进包（体积翻倍）。
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    // Rust 编译出的 libmneme_ffi.so 放到 app/src/main/jniLibs/<abi>/ 即可被打包，
    // AGP 默认已把 src/main/jniLibs 作为 jniLibs 源目录，无需额外声明。
    // 注意：不要引入 Room（本地 SQLite 由 Rust store 管理），不要引入 Retrofit（网络同步在 Rust 侧）。
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.extended)
    implementation(libs.androidx.work.runtime.ktx)
    implementation(libs.mlkit.text.recognition.chinese)
    implementation(libs.kotlinx.coroutines.android)
    // uniffi 生成的绑定用 com.sun.jna 做 FFI，必须取 @aar 变体（内含各 ABI 的 libjnidispatch.so）
    implementation(libs.jna) { artifact { type = "aar" } }

    debugImplementation(libs.androidx.compose.ui.tooling)

    testImplementation(libs.junit)
    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.androidx.espresso.core)
}
