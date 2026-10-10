plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "com.unoone.agent.vault"
    compileSdk = 35
    ndkVersion = "27.2.12479018"
    sourceSets.getByName("main").jniLibs.srcDir(layout.buildDirectory.dir("generated/vaultJni"))

    defaultConfig {
        minSdk = 28
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    packagingOptions {
        // BouncyCastle's multi-release OSGi manifest collides with jars that
        // already carry the same OSGi metadata.
        resources.excludes += "META-INF/versions/9/OSGI-INF/MANIFEST.MF"
    }
}

// Deliberately build the real arm64 library, never silently package a fake or
// stale prebuilt. Pinned Rust/NDK + --locked --offline; provision dependencies
// and rustup target aarch64-linux-android before invoking Gradle.
val vaultRepo = rootProject.projectDir.resolve("../..").canonicalFile
val nativeVaultOutput = layout.buildDirectory.dir("generated/vaultJni")
val buildNativeVault by tasks.registering(Exec::class) {
    val ndk = providers.environmentVariable("ANDROID_NDK_HOME").orNull
        ?: android.sdkDirectory.resolve("ndk/27.2.12479018").absolutePath
    inputs.files(vaultRepo.resolve("Cargo.toml"), vaultRepo.resolve("Cargo.lock"))
    inputs.dir(vaultRepo.resolve("packages/android-vault-jni"))
    inputs.dir(vaultRepo.resolve("packages/vault-core/src"))
    inputs.file(vaultRepo.resolve("packages/vault-core/Cargo.toml"))
    inputs.property("rustToolchain", "1.99.0")
    inputs.property("ndkVersion", "27.2.12479018")
    outputs.dir(nativeVaultOutput)
    commandLine("bash", vaultRepo.resolve("packages/android-vault-jni/scripts/build-android.sh"),
        nativeVaultOutput.get().asFile.absolutePath,
        layout.buildDirectory.dir("rust-vault-target").get().asFile.absolutePath, ndk)
}
tasks.named("preBuild").configure { dependsOn(buildNativeVault) }
tasks.matching { it.name.startsWith("merge") && it.name.endsWith("JniLibFolders") }
    .configureEach { dependsOn(buildNativeVault) }

val nativeVaultHost = layout.buildDirectory.dir("rust-vault-host")
val buildNativeVaultHost by tasks.registering(Exec::class) {
    inputs.files(vaultRepo.resolve("Cargo.toml"), vaultRepo.resolve("Cargo.lock"))
    inputs.dir(vaultRepo.resolve("packages/android-vault-jni"))
    inputs.dir(vaultRepo.resolve("packages/vault-core/src"))
    inputs.file(vaultRepo.resolve("packages/vault-core/Cargo.toml"))
    outputs.dir(nativeVaultHost.map { it.dir("debug") })
    commandLine("cargo", "+1.99.0", "build", "--manifest-path", vaultRepo.resolve("Cargo.toml"),
        "--locked", "--offline", "-j1", "-p", "unoone-android-vault-jni", "--target-dir", nativeVaultHost.get().asFile)
}
tasks.withType<Test>().configureEach {
    dependsOn(buildNativeVaultHost)
    jvmArgs("-Djava.library.path=${nativeVaultHost.get().asFile}/debug")
    // Existing Bouncy Castle reference-vector tests still exercise Java Argon2.
    maxHeapSize = "1400m"
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.documentfile:documentfile:1.0.1")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.0")
    testImplementation("junit:junit:4.13.2")
    // Vault unlock (Argon2id + XChaCha20-Poly1305) in production, and the
    // cross-platform contract vectors in tests.
    implementation("org.bouncycastle:bcprov-jdk18on:1.80")
}
