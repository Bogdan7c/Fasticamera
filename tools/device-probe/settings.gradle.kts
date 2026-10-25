// Зонд разведки телефона (сессия 00 плана user/foundation).
// Отдельный Gradle-проект: не часть продукта, живёт только в tools/device-probe.

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

rootProject.name = "fasticamera-device-probe"
include(":app")
