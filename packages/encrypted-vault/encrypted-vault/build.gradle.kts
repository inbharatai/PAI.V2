plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
}

group = "com.unoone.pai"
version = "0.1.0"

dependencies {
    // Core contracts (from included build)
    implementation("com.unoone.pai:core-contracts:0.1.0")

    // Encryption — using JDK built-in AES-256-GCM + HKDF-SHA256
    // BouncyCastle only needed for Argon2id KDF
    implementation("org.bouncycastle:bcprov-jdk18on:1.80")

    // Serialization
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.8.1")
    implementation("org.jetbrains.kotlinx:kotlinx-datetime:0.6.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")

    // Testing
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}

tasks.test {
    useJUnitPlatform()
}

kotlin {
    jvmToolchain(17)
}