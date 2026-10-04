plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.eroideches.omnidrop"
    // permission_handler_android is compiled against API 37 (compileSdk only, targetSdk unchanged).
    compileSdk = 37
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "com.eroideches.omnidrop"
        // Android 8.0 (Oreo) and newer.
        minSdk = 26
        targetSdk = flutter.targetSdkVersion
        // With --split-per-abi Flutter adds 1000 * ABI index to this version code.
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // Release APKs are produced unsigned (no keys or passwords in the repository or in CI):
            // sign them locally with tools/firma-release.sh.
            signingConfig = null
            isMinifyEnabled = false
            isShrinkResources = false
        }
    }

    packaging {
        // Keep native libraries uncompressed and page-aligned so libomnidrop_core.so is mapped
        // directly from the APK.
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

dependencies {
    implementation("androidx.core:core:1.16.0")
}

flutter {
    source = "../.."
}

// The Rust engine (native/) is cross-compiled with cargo-ndk into src/main/jniLibs
// (scripts/build-native.sh android). Fail early with a clear message when it is missing.
val requiredAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64")
val verifyRustLibs by tasks.registering {
    val jniDir = layout.projectDirectory.dir("src/main/jniLibs")
    doLast {
        val missing = requiredAbis.filter { !jniDir.file("$it/libomnidrop_core.so").asFile.exists() }
        if (missing.isNotEmpty()) {
            throw GradleException(
                "libomnidrop_core.so missing for $missing. Build the Rust core first: scripts/build-native.sh android",
            )
        }
    }
}
tasks.matching { it.name == "preBuild" }.configureEach { dependsOn(verifyRustLibs) }
