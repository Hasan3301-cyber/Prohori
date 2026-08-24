// Gradle settings for the Android shell.
//
// The Rust workspace is not a Gradle module — it is built by `cargo ndk` from tasks in
// `app/build.gradle.kts`. Keeping the two build systems side by side rather than nesting
// one inside the other means `cargo test` still works with no Android toolchain present,
// which is what lets CI verify every safety invariant on a plain Linux runner.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "prohori"
include(":app")
