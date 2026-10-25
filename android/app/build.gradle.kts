import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application")
}

/** Обязательная настройка из gradle.properties: отсутствие — понятная ошибка конфигурации. */
fun requiredProperty(name: String): String =
    providers.gradleProperty(name).orNull ?: error("в android/gradle.properties нет настройки $name")

/** Настройка, которая может быть пустой (например, список фич Cargo для release). */
fun optionalProperty(name: String): String = providers.gradleProperty(name).orNull.orEmpty()

/** Корень репозитория = корень Cargo workspace (android/ лежит внутри него). */
val repositoryRoot: Directory = rootProject.layout.projectDirectory.dir("..")

android {
    // namespace фиксирован: от него зависят имена JNI-символов (Java_dev_fasticamera_NativeBridge_*).
    namespace = "dev.fasticamera"
    compileSdk = 36
    ndkVersion = requiredProperty("fasticamera.ndkVersion")

    defaultConfig {
        // Решение владельца 20: своё имя пакета.
        applicationId = "dev.fasticamera"
        // Один телефон (OnePlus 11, Android 16): старые версии не поддерживаем.
        minSdk = 36
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildFeatures {
        buildConfig = true
    }

    buildTypes {
        // Debug и release стоят на телефоне рядом (решение владельца, сессия 01).
        getByName("debug") {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
            manifestPlaceholders["appLabel"] = "Fasticamera debug"
            buildConfigField("String", "LOG_FILTER", "\"${requiredProperty("fasticamera.logFilter.debug")}\"")
            ndk {
                debugSymbolLevel = "FULL"
            }
        }
        getByName("release") {
            manifestPlaceholders["appLabel"] = "Fasticamera"
            buildConfigField("String", "LOG_FILTER", "\"${requiredProperty("fasticamera.logFilter.release")}\"")
            // Личное приложение, в магазин не публикуется: подпись debug-ключом
            // (смена ключа потом — одна переустановка). Kotlin-код крошечный, R8 не нужен.
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = false
            ndk {
                // .so в APK урезаны AGP; полные символы для ndk-stack —
                // app/build/outputs/native-debug-symbols/release/native-debug-symbols.zip.
                debugSymbolLevel = "FULL"
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

/**
 * Сборка нативной библиотеки `libfasticamera.so` через cargo-ndk для одного варианта APK.
 * Результат подключается к APK как сгенерированный каталог jniLibs через Variant API —
 * так Gradle сам ставит задачу перед `merge<Variant>JniLibFolders`.
 */
abstract class CargoNdkBuildTask : DefaultTask() {
    @get:Inject
    abstract val execOperations: ExecOperations

    /** Корень Cargo workspace — рабочая папка cargo (там же rust-toolchain.toml). */
    @get:Internal
    abstract val workspaceDirectory: DirectoryProperty

    /** Исходники и манифесты Rust: входы задачи (target/ сюда не попадает). */
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val rustSources: ConfigurableFileCollection

    @get:Input
    abstract val cargoPackage: Property<String>

    @get:Input
    abstract val cargoProfile: Property<String>

    /** Фичи Cargo через запятую; пусто — без фич. */
    @get:Input
    abstract val cargoFeatures: Property<String>

    @get:Input
    abstract val platformApi: Property<String>

    /** Ревизия git, которую библиотека сообщает в отчёте `hello` (FASTICAMERA_GIT_REVISION). */
    @get:Input
    abstract val gitRevision: Property<String>

    @get:Internal
    abstract val ndkDirectory: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDirectory: DirectoryProperty

    @TaskAction
    fun build() {
        val featureArguments = cargoFeatures.get().takeIf { it.isNotBlank() }
            ?.let { listOf("--features", it) }
            .orEmpty()
        execOperations.exec {
            workingDir = workspaceDirectory.get().asFile
            environment("ANDROID_NDK_HOME", ndkDirectory.get().asFile.absolutePath)
            environment("FASTICAMERA_GIT_REVISION", gitRevision.get())
            commandLine(
                listOf(
                    "cargo", "ndk",
                    "-t", "arm64-v8a",
                    "-P", platformApi.get(),
                    "-o", outputDirectory.get().asFile.absolutePath,
                    "build",
                    "--locked",
                    "-p", cargoPackage.get(),
                    "--profile", cargoProfile.get(),
                ) + featureArguments,
            )
        }
    }
}

/** Итог git-команды: вывод или `null`, если git недоступен или команда упала (Provider не хранит null). */
data class GitCommandResult(val output: String?)

fun gitOutput(vararg arguments: String): Provider<GitCommandResult> {
    val execution = providers.exec {
        workingDir = repositoryRoot.asFile
        commandLine(listOf("git") + arguments)
        isIgnoreExitValue = true
    }
    return execution.result.zip(execution.standardOutput.asText) { result, output ->
        GitCommandResult(if (result.exitValue == 0) output.trim() else null)
    }
}

/**
 * Короткий SHA HEAD; `-dirty`, если есть любые изменения, включая неотслеживаемые файлы
 * (`git describe --dirty` их не видит, и сборка с новыми файлами выдавала бы себя за чистый
 * коммит). Вне git — честное `unknown`.
 */
val gitRevisionProvider: Provider<String> =
    gitOutput("rev-parse", "--short=12", "HEAD").zip(gitOutput("status", "--porcelain")) { revision, changes ->
        val revisionText = revision.output
        val changesText = changes.output
        when {
            revisionText == null || changesText == null -> "unknown"
            changesText.isEmpty() -> revisionText
            else -> "$revisionText-dirty"
        }
    }

androidComponents {
    onVariants { variant ->
        val buildTypeName = variant.buildType ?: error("у варианта ${variant.name} нет buildType")
        val taskName = "cargoNdkBuild" + variant.name.replaceFirstChar { it.uppercase() }
        val cargoTask = tasks.register<CargoNdkBuildTask>(taskName) {
            group = "build"
            description = "Собирает crates/fasticamera-app в libfasticamera.so (arm64-v8a) для ${variant.name}."
            workspaceDirectory.set(repositoryRoot)
            rustSources.from(
                repositoryRoot.file("Cargo.toml"),
                repositoryRoot.file("Cargo.lock"),
                repositoryRoot.file("rust-toolchain.toml"),
                repositoryRoot.dir("crates").asFileTree.matching { exclude("**/target/**") },
            )
            cargoPackage.set("fasticamera-app")
            cargoProfile.set(requiredProperty("fasticamera.cargoProfile.$buildTypeName"))
            cargoFeatures.set(optionalProperty("fasticamera.cargoFeatures.$buildTypeName"))
            platformApi.set(requiredProperty("fasticamera.rustPlatformApi"))
            gitRevision.set(gitRevisionProvider)
            ndkDirectory.set(androidComponents.sdkComponents.ndkDirectory)
            outputDirectory.set(layout.buildDirectory.dir("rust/${variant.name}/jniLibs"))
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoTask, CargoNdkBuildTask::outputDirectory)
    }
}
