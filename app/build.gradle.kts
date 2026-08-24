import org.gradle.api.tasks.Exec
import org.gradle.api.tasks.Sync
// Imported rather than written as `java.util.Properties`, because inside a Gradle Kotlin
// script `java` resolves to the Java plugin extension and shadows the package name.
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// ---------------------------------------------------------------------------
// Rust core
// ---------------------------------------------------------------------------
//
// The Gradle root and the Cargo workspace root are the same directory, so the Rust core
// is built from here rather than vendored as a prebuilt artifact. Nothing in this file
// is required to run the safety tests — `cargo test --workspace` works with no Android
// toolchain installed, which is what lets CI verify every invariant on a plain runner.

val rustRoot: File = rootProject.projectDir

val androidAbis: List<String> =
    (project.findProperty("prohoriAbis") as String? ?: "arm64-v8a")
        .split(",")
        .map { it.trim() }
        .filter { it.isNotEmpty() }

val rustJniLibs: Provider<Directory> = layout.buildDirectory.dir("rustJniLibs")
val uniffiKotlin: Provider<Directory> = layout.buildDirectory.dir("generated/uniffi")

// Ship the verified Q4 model with sideload builds. Android cannot hand llama.cpp a regular
// filesystem path inside an APK, so the app copies this uncompressed asset to private storage
// on first launch. Staging it under build/ avoids committing a second 1.1 GB copy.
val bundledModelSource = rootProject.file("model/artifacts/Qwen3-1.7B-Q4_K_M.gguf")
val bundledModelAssets: Provider<Directory> = layout.buildDirectory.dir("generated/bundledModelAssets")
val prepareBundledModel by tasks.registering(Sync::class) {
    group = "model"
    description = "Stages the verified Qwen3 Q4 GGUF for APK packaging"
    inputs.file(bundledModelSource)
    outputs.dir(bundledModelAssets)
    from(bundledModelSource) {
        into("models")
        rename { "qwen3-1.7b-q4_k_m.gguf" }
    }
    into(bundledModelAssets)
    doFirst {
        require(bundledModelSource.isFile) {
            "Bundled model is missing: ${bundledModelSource.absolutePath}"
        }
        require(bundledModelSource.length() == 1_107_408_544L) {
            "Bundled model size does not match the verified Q4 artifact"
        }
    }
}

// llama.cpp is fetched at one reviewed commit into build/ (never into the source tree).
// Keeping the commit here and in tools/fetch-llama.ps1 makes upgrades explicit and makes
// a clean checkout reproducible without committing hundreds of megabytes of third-party
// source. The build stages the locally verified model and fails clearly when it is absent.
// Release b10516, published 2026-08-20. model/model.lock.json pins the matching
// host tools, so desktop probes and the APK cannot silently use different APIs.
val llamaCppCommit = "b95502ba9aa0eb73a2f4fc8878d7fbe6a847a0b9"
val llamaCppSource: Directory = rootProject.layout.projectDirectory.dir("third_party/llama.cpp")
val fetchLlamaScript =
    if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
        file("$rustRoot/tools/fetch-llama.ps1")
    } else {
        file("$rustRoot/tools/fetch-llama.sh")
    }

val prepareLlamaCpp by tasks.registering(Exec::class) {
    group = "native"
    description = "Fetches the pinned llama.cpp source used by the on-device engine"
    workingDir = rustRoot
    inputs.property("commit", llamaCppCommit)
    inputs.file(fetchLlamaScript)
    // Snapshot only the checkout identity and root build file. Hashing the full third-party
    // tree made Gradle spend minutes walking files that Git already pins immutably.
    outputs.files(llamaCppSource.file(".git/HEAD"), llamaCppSource.file("CMakeLists.txt"))
    if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
        commandLine(
            "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
            fetchLlamaScript.absolutePath, "-Destination", llamaCppSource.asFile.absolutePath,
            "-Commit", llamaCppCommit,
        )
    } else {
        commandLine("bash", fetchLlamaScript.absolutePath, llamaCppSource.asFile.absolutePath, llamaCppCommit)
    }
}

/** Everything a change to which should trigger a rebuild of the core. */
val rustSources: FileCollection =
    files(
        "$rustRoot/Cargo.toml",
        "$rustRoot/Cargo.lock",
        "$rustRoot/core-ffi/Cargo.toml",
        "$rustRoot/core-ffi/uniffi.toml",
        "$rustRoot/core/Cargo.toml",
    ) +
        fileTree("$rustRoot/core/src") +
        fileTree("$rustRoot/core-ffi/src") +
        // The corpus is compiled into the library (`core/src/bundled.rs`), so editing a
        // protocol has to rebuild the .so. Leaving this out would produce an app running
        // yesterday's medical text with no sign that anything was stale.
        fileTree("$rustRoot/data/firstaid") +
        fileTree("$rustRoot/data/grammar") +
        fileTree("$rustRoot/data/prompts")

/**
 * Cross-compile `prohori-ffi` for Android.
 *
 * Always `--release`, including for debug APKs. The timing budget in `PLAN.md` §8 (a
 * red-flag card in under 100 ms) is only meaningful against the profile that ships, and a
 * debug `.so` would miss it quietly while the app still looked fine.
 */
val cargoBuildAndroid by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds libprohori_ffi.so for ${androidAbis.joinToString()}"
    workingDir = rustRoot
    inputs.files(rustSources)
    outputs.dir(rustJniLibs)

    val command = mutableListOf("cargo", "ndk")
    androidAbis.forEach { abi -> command += listOf("-t", abi) }
    command += listOf("-o", rustJniLibs.get().asFile.absolutePath)
    command += listOf("build", "--release", "--locked", "-p", "prohori-ffi")
    commandLine(command)

    doFirst {
        rustJniLibs.get().asFile.mkdirs()
        logger.lifecycle("cargo-ndk: ${command.joinToString(" ")}")
    }
}

/**
 * Generate the Kotlin bindings from the library that was just built.
 *
 * Read from the compiled `.so` rather than from the source tree on purpose: the bindings
 * then describe the exact library the APK will load. A binding generated from source
 * while a stale `.so` shipped alongside it compiles cleanly and then misreads the FFI
 * buffer at runtime, which is the worst class of bug this boundary can have.
 */
val generateUniffiBindings by tasks.registering(Exec::class) {
    group = "rust"
    description = "Generates org.prohori.core Kotlin bindings from the built library"
    dependsOn(cargoBuildAndroid)
    workingDir = rustRoot
    inputs.dir(rustJniLibs)
    outputs.dir(uniffiKotlin)

    val library = File(rustJniLibs.get().asFile, "${androidAbis.first()}/libprohori_ffi.so")
    commandLine(
        "cargo",
        "run",
        "--quiet",
        "--locked",
        "-p",
        "prohori-uniffi-bindgen",
        "--",
        "generate",
        "--library",
        library.absolutePath,
        "--language",
        "kotlin",
        // ktlint is not a build dependency here; the generated file is never hand-edited.
        "--no-format",
        "--out-dir",
        uniffiKotlin.get().asFile.absolutePath,
    )
}

/**
 * The Rust safety suite, wired into `./gradlew check`.
 *
 * An Android build that passes while the red-flag tests fail is a build that says the app
 * is fine when its only safety layer is not.
 */
val cargoTest by tasks.registering(Exec::class) {
    group = "verification"
    description = "Runs the Rust safety suite"
    workingDir = rustRoot
    commandLine("cargo", "test", "--workspace", "--locked")
}

tasks.named("check") { dependsOn(cargoTest) }

// Kotlin compilation needs the generated bindings to exist first. Matched by name so this
// file does not have to import the Kotlin plugin's task types.
tasks.matching { it.name.startsWith("compile") && it.name.endsWith("Kotlin") }
    .configureEach { dependsOn(generateUniffiBindings) }

tasks.named("preBuild") {
    dependsOn(generateUniffiBindings)
    dependsOn(prepareBundledModel)
}
tasks.matching { it.name.startsWith("merge") && it.name.endsWith("Assets") }
    .configureEach { dependsOn(prepareBundledModel) }

// CMake configures before Kotlin compilation, so its generated task also needs the pinned
// source. Matching the stable Android task prefix avoids coupling this file to AGP internals.
tasks.matching { it.name.startsWith("configureCMake") || it.name.startsWith("buildCMake") }
    .configureEach { dependsOn(prepareLlamaCpp) }

// ---------------------------------------------------------------------------
// Relay configuration
// ---------------------------------------------------------------------------
//
// The online hospital-confirmation channel talks to a relay the operator's organisation
// hosts; the relay holds the Telegram bot token and is Telegram's single `getUpdates`
// consumer. See `docs/P4.md` for why the token cannot live in the APK: Telegram answers
// 409 Conflict to a second consumer, so N installs sharing a token means the winner commits
// the offset and one phone's YES is delivered to another phone. A confirmation that
// silently reaches the wrong device is exactly the failure `PLAN.md` §7 forbids.
//
// Values come from `local.properties`, which is gitignored, or from the environment for CI.
// All three default to empty, so a plain `git clone` builds and the online button is simply
// absent — a missing feature, never a broken one.
val relayProperties =
    Properties().apply {
        val file = rootProject.file("local.properties")
        if (file.exists()) {
            file.inputStream().use { load(it) }
        }
    }

fun relaySetting(key: String): String =
    (relayProperties.getProperty(key) ?: System.getenv(key) ?: "").trim()

val relayBaseUrl = relaySetting("PROHORI_RELAY_BASE_URL")
val relayDeviceToken = relaySetting("PROHORI_RELAY_DEVICE_TOKEN")

// A real bot token, for driving one phone against Telegram directly during a demo without
// standing up a relay first. Debug builds only, and never written into a release
// BuildConfig — see the check below.
val debugBotToken = relaySetting("PROHORI_DEBUG_BOT_TOKEN")

// Quoting is not cosmetic: buildConfigField pastes its value into generated Java verbatim,
// so an unescaped value would either fail to compile or inject code.
fun javaString(value: String): String = "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

if (relayBaseUrl.isNotEmpty() && !relayBaseUrl.startsWith("https://")) {
    // Loopback is allowed because that is how a local relay is tested; anything else
    // cleartext would put a device token and a case id on the wire in the clear.
    val loopback = relayBaseUrl.startsWith("http://10.0.2.2") || relayBaseUrl.startsWith("http://localhost")
    require(loopback) {
        "PROHORI_RELAY_BASE_URL must be https:// (http:// is permitted only for 10.0.2.2 or localhost)"
    }
}

// ---------------------------------------------------------------------------
// Android
// ---------------------------------------------------------------------------

android {
    namespace = "org.prohori.app"
    // One NDK for cargo-ndk, CMake, local builds, and CI. Letting AGP choose its default
    // made it install r27c beside the checksum-verified r27d toolchain.
    ndkVersion = "27.3.13750724"
    // androidx.core 1.15.0 needs at least API 35. API 36 is the complete platform
    // installed on the build machine and is supported by AGP 8.9.1. This does not
    // change the install floor (minSdk 24) or target-34 runtime behaviour.
    compileSdk = 36

    defaultConfig {
        applicationId = "org.prohori.app"
        // API 24 (2016). The target user is not on a new phone; that is the same premise
        // behind the 1.5 GB model floor in `PLAN.md` §8.
        minSdk = 24
        // Google Play requires API 36 for new mobile apps and updates from 2026-08-31.
        // Prohori does not use background services, exact alarms, or broad storage, so
        // the API-36 behaviour change does not weaken an emergency path.
        targetSdk = 36
        versionCode = 1
        versionName = "0.7.0-dev"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += androidAbis }
        externalNativeBuild {
            cmake {
                arguments += listOf(
                    "-DPROHORI_LLAMA_SRC=${llamaCppSource.asFile.absolutePath.replace('\\', '/')}",
                    "-DLLAMA_BUILD_COMMON=OFF",
                    "-DLLAMA_BUILD_TOOLS=OFF",
                    "-DLLAMA_BUILD_TESTS=OFF",
                    "-DLLAMA_BUILD_EXAMPLES=OFF",
                    "-DLLAMA_BUILD_SERVER=OFF",
                    "-DGGML_NATIVE=OFF",
                    "-DGGML_OPENMP=OFF",
                    "-DGGML_LLAMAFILE=OFF",
                )
                cppFlags += listOf("-std=c++17", "-O3")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // A release APK is a public artifact. Fail the build rather than ship a bot
            // token that anyone can extract with `unzip | strings`.
            require(debugBotToken.isEmpty()) {
                "PROHORI_DEBUG_BOT_TOKEN is set, and a release build must not carry a bot token. " +
                    "Remove it from local.properties (and revoke it with @BotFather if it has " +
                    "ever been in a build that left this machine)."
            }
            buildConfigField("String", "RELAY_BASE_URL", javaString(relayBaseUrl))
            buildConfigField("String", "RELAY_DEVICE_TOKEN", javaString(relayDeviceToken))
            buildConfigField("String", "DEBUG_BOT_TOKEN", javaString(""))
        }
        debug {
            applicationIdSuffix = ".debug"
            buildConfigField("String", "RELAY_BASE_URL", javaString(relayBaseUrl))
            buildConfigField("String", "RELAY_DEVICE_TOKEN", javaString(relayDeviceToken))
            buildConfigField("String", "DEBUG_BOT_TOKEN", javaString(debugBotToken))
        }
    }

    sourceSets["main"].jniLibs.srcDir(rustJniLibs)
    sourceSets["main"].kotlin.srcDir(uniffiKotlin)
    sourceSets["main"].assets.srcDir(bundledModelAssets)

    androidResources {
        // llama.cpp memory-maps a normal file after first-launch extraction. Avoid spending
        // build/install time compressing weights that are already quantized binary data.
        noCompress += "gguf"
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.31.6"
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
        // Carries the relay base URL and device token. Nothing else in the app needs a
        // generated constant.
        buildConfig = true
    }
    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.android)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)

    // uniffi's generated Kotlin reaches the .so through JNA direct mapping. The `@aar`
    // variant is the one that carries JNA's own Android native libraries.
    implementation(variantOf(libs.jna) { artifactType("aar") })

    debugImplementation(libs.androidx.compose.ui.tooling)
    debugImplementation(libs.androidx.compose.ui.test.manifest)

    testImplementation(libs.junit)
    // Android's org.json classes are stubs in local JVM tests; use the matching reference
    // implementation so provider responses can be tested without a device or live API key.
    testImplementation(libs.json)
    androidTestImplementation(libs.androidx.test.junit)
    androidTestImplementation(libs.androidx.espresso.core)
    androidTestImplementation(platform(libs.androidx.compose.bom))
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
}
