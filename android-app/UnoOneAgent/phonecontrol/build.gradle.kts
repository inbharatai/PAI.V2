plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.unoone.agent.phonecontrol"
    compileSdk = 35

    defaultConfig {
        minSdk = 28
        targetSdk = 35
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation(project(":core"))
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    
    // On-device OCR via bundled ML Kit text-recognition.
    // Latin script recognizer (default — handles English and other Latin-script text).
    implementation("com.google.mlkit:text-recognition:16.0.0")
    // Indic script recognizer (Devanagari — handles Hindi, Marathi, Nepali, and other
    // Devanagari-script languages). Used alongside the Latin recognizer when the active
    // voice language is an Indic language, so Hindi/Bengali/Tamil/etc. text on screen
    // is no longer invisible to OCR. M15: describe_scene missing Indic script OCR.
    implementation("com.google.mlkit:text-recognition-devanagari:16.0.0")

    // Labeled, fully offline COCO object detection for Blind Aid.
    //noinspection GradleDependency -- Google publishes a legacy date-version that sorts above
    // the current 0.10.x line; use the current Tasks release instead.
    implementation("com.google.mediapipe:tasks-vision:0.10.35")

    // Android-compatible PDFBox fork for fully offline AcroForm inspection and save-as-copy.
    implementation("com.tom-roush:pdfbox-android:2.0.27.0") {
        // PDF signing/encryption is outside DocumentFillEngine's contract. Keeping the optional
        // Bouncy Castle stack would also package its trust-all TLS helper, which Android lint
        // correctly rejects. AcroForm inspection/filling does not require these modules.
        exclude(group = "org.bouncycastle", module = "bcprov-jdk15to18")
        exclude(group = "org.bouncycastle", module = "bcpkix-jdk15to18")
        exclude(group = "org.bouncycastle", module = "bcutil-jdk15to18")
    }

    // CameraX for real-time continuous blind aid analysis
    val cameraVersion = "1.3.3"
    implementation("androidx.camera:camera-core:$cameraVersion")
    implementation("androidx.camera:camera-camera2:$cameraVersion")
    implementation("androidx.camera:camera-lifecycle:$cameraVersion")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.12")
}
