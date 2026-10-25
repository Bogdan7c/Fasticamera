import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application")
}

// Параметры сборки Rust-части берём из gradle.properties, а не хардкодим здесь.
val probeNdkVersion = providers.gradleProperty("probe.ndkVersion").get()
val probeRustPlatformApi = providers.gradleProperty("probe.rustPlatformApi").get()

android {
    // namespace фиксирован: от него зависят имена JNI-символов (dev.fasticamera.probe.NativeBridge).
    // applicationId меняется по вариантам ниже, namespace — никогда.
    namespace = "dev.fasticamera.probe"
    compileSdk = 36
    ndkVersion = probeNdkVersion

    defaultConfig {
        applicationId = "dev.fasticamera.probe"
        minSdk = 34
        targetSdk = 36
        versionCode = 1
        versionName = "0.1-recon"
        ndk {
            // Телефон один (OnePlus 11, arm64), другие ABI не собираем.
            abiFilters += "arm64-v8a"
        }
    }

    // Варианты отличаются только именем пакета. Это и есть эксперимент C1:
    // видит ли HAL/фреймворк наш APK как «привилегированную камеру» по имени из
    // persist.camera.privapp.list.
    flavorDimensions += "identity"
    productFlavors {
        create("control") {
            dimension = "identity"
            applicationId = "dev.fasticamera.probe"
        }
        create("oneplus") {
            dimension = "identity"
            applicationId = "com.oneplus.camera"
        }
        create("oppo") {
            dimension = "identity"
            applicationId = "com.oppo.camera"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.isReturnDefaultValues = false
    }
}

/**
 * Сборка Rust-библиотеки зонда через cargo-ndk. Результат подключается к APK как
 * сгенерированный каталог jniLibs через Variant API — так Gradle сам знает зависимость задач.
 * NDK берём тот же, что у AGP, чтобы Rust и Gradle не разъехались по версиям.
 */
abstract class CargoNdkBuildTask : DefaultTask() {
    @get:Inject
    abstract val execOperations: ExecOperations

    /** Каталог crate — только рабочая папка; входы задачи — исходники ниже (target/ не хешируем). */
    @get:Internal
    abstract val crateDirectory: DirectoryProperty

    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val crateSources: ConfigurableFileCollection

    @get:Input
    abstract val platformApi: Property<String>

    @get:Internal
    abstract val ndkDirectory: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDirectory: DirectoryProperty

    @TaskAction
    fun build() {
        execOperations.exec {
            workingDir = crateDirectory.get().asFile
            environment("ANDROID_NDK_HOME", ndkDirectory.get().asFile.absolutePath)
            commandLine(
                "cargo", "ndk",
                "-t", "arm64-v8a",
                "-P", platformApi.get(),
                "-o", outputDirectory.get().asFile.absolutePath,
                "build", "--release",
            )
        }
    }
}

val cargoNdkBuild = tasks.register<CargoNdkBuildTask>("cargoNdkBuild") {
    group = "build"
    description = "Собирает tools/device-probe/native в libprobe_native.so (arm64-v8a)."
    val nativeDir = rootProject.layout.projectDirectory.dir("native")
    crateDirectory.set(nativeDir)
    crateSources.from(
        nativeDir.dir("src"),
        nativeDir.dir("shaders"),
        nativeDir.file("Cargo.toml"),
        nativeDir.file("Cargo.lock"),
        nativeDir.file("build.rs"),
    )
    platformApi.set(probeRustPlatformApi)
    ndkDirectory.set(androidComponents.sdkComponents.ndkDirectory)
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdkBuild, CargoNdkBuildTask::outputDirectory)
    }
}

dependencies {
    // org.json в unit-тестах на JVM: в android.jar только заглушки, которые бросают исключения.
    testImplementation("org.json:json:20260814")
    testImplementation("junit:junit:4.13.2")
}
