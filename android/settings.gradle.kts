// Android-оболочка Fasticamera: тонкая Kotlin-Activity + нативная библиотека из Cargo workspace
// (корень репозитория). План и решения — user/foundation/README.md.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    // Репозитории задаются только здесь, модули свои не добавляют.
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "fasticamera"
include(":app")
