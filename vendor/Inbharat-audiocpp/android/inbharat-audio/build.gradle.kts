plugins {
    id("com.android.library")
    kotlin("android")
}

android {
    namespace = "org.inbharat.audio"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
        ndk { abiFilters += "arm64-v8a" }
        externalNativeBuild {
            cmake {
                arguments += listOf(
                    "-DIBAUDIO_BUILD_CLI=OFF",
                    "-DIBAUDIO_BUILD_TESTS=OFF",
                    "-DIBAUDIO_BUILD_ANDROID_JNI=ON",
                    "-DIBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=OFF",
                    "-DIBAUDIO_ENABLE_VULKAN_PROBE=OFF",
                    "-DIBAUDIO_WARNINGS_AS_ERRORS=ON",
                    "-DANDROID_STL=c++_shared"
                )
                cppFlags += listOf("-std=c++17", "-fno-openmp")
                targets += listOf("ibaudio", "ibaudio_jni")
            }
        }
    }

    buildFeatures { buildConfig = true }
    buildTypes {
        debug { isJniDebuggable = true }
        release {
            isMinifyEnabled = false
            consumerProguardFiles("consumer-rules.pro")
        }
    }
    flavorDimensions += "backendProbe"
    productFlavors {
        create("cpu") {
            dimension = "backendProbe"
            externalNativeBuild.cmake.arguments += "-DIBAUDIO_ENABLE_VULKAN_PROBE=OFF"
        }
        create("vulkanProbe") {
            dimension = "backendProbe"
            externalNativeBuild.cmake.arguments += "-DIBAUDIO_ENABLE_VULKAN_PROBE=ON"
        }
    }
    externalNativeBuild {
        cmake {
            path = file("../../CMakeLists.txt")
            version = "3.30.5"
        }
    }
    kotlinOptions { jvmTarget = "17" }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    packaging { jniLibs.useLegacyPackaging = false }
}
