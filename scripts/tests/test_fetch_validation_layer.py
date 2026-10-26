"""Тесты scripts/fetch-validation-layer.sh без сети.

Источник «скачивания» — локальный файл через ``file://`` URL (curl его понимает), метаданные
и кэш — во временных каталогах. Проверяем результат: код выхода, что осталось в кэше, какая
библиотека извлечена, и что при валидном кэше скрипт не обращается к источнику вовсе.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "fetch-validation-layer.sh"
REPO_METADATA = SCRIPT.parent.parent / "android" / "validation-layer.toml"
LIBRARY_MEMBER = "arm64-v8a/libVkLayer_khronos_validation.so"
LIBRARY_BYTES = b"\x7fELF-fake-validation-layer"
EXIT_OK = 0
EXIT_DOWNLOAD_FAILED = 5
EXIT_CHECKSUM_MISMATCH = 6
EXIT_TOOLING_OR_METADATA = 7
EXIT_USAGE = 64


def build_fake_release_archive(path: Path, *, with_library: bool = True) -> str:
    """Собирает zip как у Khronos (корневой каталог + <abi>/lib*.so) и возвращает его SHA-256."""
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("android-binaries-9.9.9.9/x86_64/libVkLayer_khronos_validation.so", b"other-abi")
        if with_library:
            archive.writestr(f"android-binaries-9.9.9.9/{LIBRARY_MEMBER}", LIBRARY_BYTES)
    return hashlib.sha256(path.read_bytes()).hexdigest()


class FetchValidationLayerTest(unittest.TestCase):
    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.addCleanup(self._temp.cleanup)
        self.root = Path(self._temp.name)
        self.cache_home = self.root / "cache"
        self.source_archive = self.root / "source.zip"
        self.version_dir = self.cache_home / "fasticamera" / "vulkan-validation-layers" / "9.9.9.9"
        self.cached_archive = self.version_dir / "android-binaries-9.9.9.9.zip"
        self.extracted_library = self.version_dir / "jniLibs" / LIBRARY_MEMBER

    def write_metadata(self, sha256: str, url: str | None = None) -> Path:
        metadata = self.root / "validation-layer.toml"
        source_url = url if url is not None else self.source_archive.as_uri()
        metadata.write_text(
            f'version = "9.9.9.9"\nurl = "{source_url}"\nsha256 = "{sha256}"\narchive_member = "{LIBRARY_MEMBER}"\n',
            encoding="utf-8",
        )
        return metadata

    def run_script(self, metadata: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
        environment = {
            **os.environ,
            "FASTICAMERA_VALIDATION_LAYER_METADATA": str(metadata),
            "XDG_CACHE_HOME": str(self.cache_home),
        }
        return subprocess.run(
            ["bash", str(SCRIPT), *arguments], env=environment, capture_output=True, text=True, check=False
        )

    def test_downloads_verifies_and_extracts_only_the_arm64_library(self) -> None:
        sha256 = build_fake_release_archive(self.source_archive)
        result = self.run_script(self.write_metadata(sha256))
        self.assertEqual(EXIT_OK, result.returncode, result.stderr)
        self.assertEqual(LIBRARY_BYTES, self.extracted_library.read_bytes())
        # Другие ABI в каталог jniLibs не попадают: иначе они бы уехали в APK.
        self.assertEqual([self.version_dir / "jniLibs" / "arm64-v8a"], sorted((self.version_dir / "jniLibs").iterdir()))

    def test_checksum_mismatch_fails_and_purges_the_cache(self) -> None:
        build_fake_release_archive(self.source_archive)
        result = self.run_script(self.write_metadata("0" * 64))
        self.assertEqual(EXIT_CHECKSUM_MISMATCH, result.returncode, result.stderr)
        self.assertFalse(self.version_dir.exists(), "после несовпадения кэш версии должен быть удалён")
        self.assertIn("SHA-256", result.stderr)

    def test_cached_archive_is_used_without_touching_the_source(self) -> None:
        sha256 = build_fake_release_archive(self.source_archive)
        metadata = self.write_metadata(sha256)
        self.assertEqual(EXIT_OK, self.run_script(metadata).returncode)
        # Источник исчез: повторный запуск обязан обойтись кэшем (офлайн).
        self.source_archive.unlink()
        self.extracted_library.unlink()
        result = self.run_script(metadata)
        self.assertEqual(EXIT_OK, result.returncode, result.stderr)
        self.assertEqual(LIBRARY_BYTES, self.extracted_library.read_bytes())

    def test_corrupted_cached_archive_is_rejected_and_removed(self) -> None:
        sha256 = build_fake_release_archive(self.source_archive)
        metadata = self.write_metadata(sha256)
        self.assertEqual(EXIT_OK, self.run_script(metadata).returncode)
        self.cached_archive.write_bytes(b"tampered")
        result = self.run_script(metadata)
        self.assertEqual(EXIT_CHECKSUM_MISMATCH, result.returncode, result.stderr)
        self.assertFalse(self.cached_archive.exists())

    def test_unreachable_source_reports_download_failure_and_leaves_no_partial_cache(self) -> None:
        result = self.run_script(self.write_metadata("0" * 64, url=(self.root / "missing.zip").as_uri()))
        self.assertEqual(EXIT_DOWNLOAD_FAILED, result.returncode, result.stderr)
        self.assertEqual([], list(self.version_dir.glob("*.zip*")))

    def test_archive_without_the_library_is_reported_not_silently_accepted(self) -> None:
        sha256 = build_fake_release_archive(self.source_archive, with_library=False)
        result = self.run_script(self.write_metadata(sha256))
        self.assertEqual(EXIT_TOOLING_OR_METADATA, result.returncode, result.stderr)
        self.assertFalse(self.extracted_library.exists())

    def test_missing_metadata_key_is_reported(self) -> None:
        metadata = self.root / "broken.toml"
        metadata.write_text('version = "9.9.9.9"\n', encoding="utf-8")
        result = self.run_script(metadata)
        self.assertEqual(EXIT_TOOLING_OR_METADATA, result.returncode, result.stderr)
        self.assertIn("url", result.stderr)

    def test_print_dir_only_prints_and_does_not_touch_cache(self) -> None:
        result = self.run_script(self.write_metadata("0" * 64), "--print-dir")
        self.assertEqual(EXIT_OK, result.returncode, result.stderr)
        self.assertEqual(str(self.version_dir / "jniLibs"), result.stdout.strip())
        self.assertFalse(self.version_dir.exists())

    def test_unknown_argument_is_usage_error(self) -> None:
        result = self.run_script(self.write_metadata("0" * 64), "--bogus")
        self.assertEqual(EXIT_USAGE, result.returncode)

    def test_repository_metadata_is_well_formed(self) -> None:
        """Закреплённые метаданные репозитория читаются скриптом и имеют похожий на SHA-256 хэш."""
        result = self.run_script(REPO_METADATA, "--print-dir")
        self.assertEqual(EXIT_OK, result.returncode, result.stderr)
        text = REPO_METADATA.read_text(encoding="utf-8")
        self.assertRegex(text, r'(?m)^sha256 = "[0-9a-f]{64}"$')
        self.assertIn("vulkan-sdk-", text)


if __name__ == "__main__":
    unittest.main()
