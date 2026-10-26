"""Тесты ожиданий сценариев lifecycle/permission/motion/platform: от строк logcat до вердикта."""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS_DIR))

import smoke_checks  # noqa: E402  (путь к scripts/ добавлен строкой выше)
import smoke_wait  # noqa: E402

GOOD_EVENTS = ["paused", "surface_destroyed", "surface_created", "surface_changed", "resumed"]


def result_line(payload: dict) -> str:
    return f"  1792957061.123  4242  4242 I Fasticamera: FASTICAMERA_RESULT {json.dumps(payload)}\n"


def lifecycle_payload(**report_overrides: object) -> dict:
    report = {
        "events": list(GOOD_EVENTS),
        "live_global_refs_at_ready": 12,
        "live_global_refs_at_end": 12,
        "surface_release_timeouts": 0,
        "activity_recreated": False,
        "touch_samples_dropped": 0,
    }
    report.update(report_overrides)
    return {"scenario": "lifecycle", "status": "passed", "report": report}


def motion_payload(**gyro_overrides: object) -> dict:
    gyro = {"samples": 1200, "rate_hz": 401.7, "max_gap_ms": 3.1, "median_delivery_lag_ms": 2.6, "requested_rate_hz": 400}
    gyro.update(gyro_overrides)
    report = {"gyro": gyro, "orientation": "portrait", "accelerometer_samples": 300}
    return {"scenario": "motion", "status": "passed", "report": report}


def platform_payload() -> dict:
    report = {
        "thermal": {"status": "none", "headroom": 0.46, "listener": "registered"},
        "haptics": {
            "vibrator": "present",
            "patterns": [
                {"pattern": "selection_tick", "realization": "predefined_effect", "outcome": "queued"},
                {"pattern": "shutter", "realization": "primitive", "outcome": "queued"},
            ],
        },
        "frame_rate": {"requested_hz": 120.0, "result": "applied"},
    }
    return {"scenario": "platform", "status": "passed", "report": report}


def permission_payload(answers: list[str]) -> dict:
    return {"scenario": "permission", "status": "passed", "report": {"answers": answers}}


def verdict_for(payload: dict):
    return smoke_wait.evaluate_stream([result_line(payload)], payload["scenario"])


class LifecycleOrderTest(unittest.TestCase):
    def test_good_order_has_no_problems(self) -> None:
        self.assertEqual(smoke_checks.lifecycle_order_problems(GOOD_EVENTS), [])

    def test_ignored_events_between_lifecycle_events_do_not_matter(self) -> None:
        events = ["touch", "paused", "thermal_status", "surface_destroyed", "device_orientation", "surface_created",
                  "touch_gap", "resumed", "camera_permission", "platform_command_failed"]
        self.assertEqual(smoke_checks.lifecycle_order_problems(events), [])

    def test_resumed_before_surface_created_is_accepted(self) -> None:
        # Так приходит на OnePlus 11 (сессия 02): Android порядок при возврате не гарантирует.
        events = ["paused", "surface_destroyed", "resumed", "surface_created", "surface_changed"]
        self.assertEqual(smoke_checks.lifecycle_order_problems(events), [])

    def test_missing_resumed_after_return_is_reported(self) -> None:
        events = ["paused", "surface_destroyed", "surface_created", "surface_changed"]
        problems = smoke_checks.lifecycle_order_problems(events)
        self.assertTrue(any("нет 'resumed'" in problem for problem in problems), problems)

    def test_resumed_before_background_is_reported(self) -> None:
        events = ["paused", "resumed", "surface_destroyed", "surface_created", "resumed"]
        problems = smoke_checks.lifecycle_order_problems(events)
        self.assertTrue(any("'resumed' раньше 'surface_destroyed'" in problem for problem in problems), problems)

    def test_must_start_with_paused(self) -> None:
        problems = smoke_checks.lifecycle_order_problems(["surface_destroyed", "paused", "surface_created", "resumed"])
        self.assertTrue(any("ожидалось 'paused'" in problem for problem in problems), problems)

    def test_missing_surface_destroyed_is_reported(self) -> None:
        problems = smoke_checks.lifecycle_order_problems(["paused", "resumed"])
        self.assertTrue(any("нет 'surface_destroyed'" in problem for problem in problems), problems)

    def test_surface_created_before_destroyed_is_reported(self) -> None:
        events = ["paused", "surface_created", "surface_destroyed", "surface_created", "resumed"]
        problems = smoke_checks.lifecycle_order_problems(events)
        self.assertTrue(any("раньше 'surface_destroyed'" in problem for problem in problems), problems)

    def test_no_surface_recreation_is_reported(self) -> None:
        problems = smoke_checks.lifecycle_order_problems(["paused", "surface_destroyed", "resumed"])
        self.assertTrue(any("нет 'surface_created'" in problem for problem in problems), problems)

    def test_surface_changed_before_created_is_reported(self) -> None:
        events = ["paused", "surface_destroyed", "surface_changed", "surface_created", "resumed"]
        problems = smoke_checks.lifecycle_order_problems(events)
        self.assertTrue(any("'surface_changed' раньше" in problem for problem in problems), problems)

    def test_no_lifecycle_events_is_reported(self) -> None:
        problems = smoke_checks.lifecycle_order_problems(["touch", "thermal_status"])
        self.assertTrue(any("нет событий" in problem for problem in problems), problems)


class LifecycleVerdictTest(unittest.TestCase):
    def assert_fails_with(self, payload: dict, expected_text: str) -> None:
        verdict = verdict_for(payload)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
        self.assertIn(expected_text, verdict.message)

    def test_good_report_passes(self) -> None:
        verdict = verdict_for(lifecycle_payload())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_wrong_event_order_fails_with_order_message(self) -> None:
        self.assert_fails_with(lifecycle_payload(events=["paused", "resumed"]), "surface_destroyed")

    def test_leaked_global_refs_fail(self) -> None:
        self.assert_fails_with(lifecycle_payload(live_global_refs_at_end=15), "утечка глобальных JNI-ссылок: было 12, стало 15")

    def test_missing_ref_counters_fail(self) -> None:
        payload = lifecycle_payload()
        del payload["report"]["live_global_refs_at_end"]
        self.assert_fails_with(payload, "нет счётчиков")

    def test_surface_release_timeout_fails(self) -> None:
        self.assert_fails_with(lifecycle_payload(surface_release_timeouts=1), "surface_release_timeouts=1")

    def test_activity_recreated_fails(self) -> None:
        self.assert_fails_with(lifecycle_payload(activity_recreated=True), "activity_recreated=True")

    def test_missing_activity_recreated_fails(self) -> None:
        payload = lifecycle_payload()
        del payload["report"]["activity_recreated"]
        self.assert_fails_with(payload, "activity_recreated=None")

    def test_failed_status_is_reported(self) -> None:
        self.assert_fails_with({"scenario": "lifecycle", "status": "failed", "error": "нет surface"}, "нет surface")


class PermissionVerdictTest(unittest.TestCase):
    def test_denied_then_permanently_denied_passes(self) -> None:
        verdict = verdict_for(permission_payload(["denied", "denied_permanently", "denied_permanently"]))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_granted_answer_fails(self) -> None:
        verdict = verdict_for(permission_payload(["granted", "granted", "granted"]))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("ответы на запросы разрешения", verdict.message)

    def test_wrong_order_fails(self) -> None:
        verdict = verdict_for(permission_payload(["denied_permanently", "denied", "denied_permanently"]))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)

    def test_missing_third_answer_fails(self) -> None:
        verdict = verdict_for(permission_payload(["denied", "denied_permanently"]))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)

    def test_failed_status_is_reported(self) -> None:
        verdict = verdict_for({"scenario": "permission", "status": "failed", "error": "запрос не ушёл"})
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("запрос не ушёл", verdict.message)


class MotionVerdictTest(unittest.TestCase):
    def test_good_report_passes(self) -> None:
        self.assertEqual(verdict_for(motion_payload()).exit_code, smoke_wait.EXIT_OK)

    def test_rate_exactly_at_threshold_passes(self) -> None:
        verdict = verdict_for(motion_payload(rate_hz=smoke_checks.MIN_GYRO_RATE_HZ))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_slow_gyro_fails(self) -> None:
        verdict = verdict_for(motion_payload(rate_hz=379.9))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("ниже порога", verdict.message)

    def test_zero_gyro_samples_fail(self) -> None:
        verdict = verdict_for(motion_payload(samples=0))
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("нет отсчётов гироскопа", verdict.message)

    def test_missing_accelerometer_samples_fail(self) -> None:
        payload = motion_payload()
        payload["report"]["accelerometer_samples"] = 0
        verdict = verdict_for(payload)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("нет отсчётов акселерометра", verdict.message)

    def test_missing_rate_fails(self) -> None:
        payload = motion_payload()
        del payload["report"]["gyro"]["rate_hz"]
        verdict = verdict_for(payload)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED)
        self.assertIn("нет частоты гироскопа", verdict.message)


class PlatformVerdictTest(unittest.TestCase):
    def failing_verdict(self, mutate) -> smoke_wait.SmokeVerdict:
        payload = copy.deepcopy(platform_payload())
        mutate(payload["report"])
        verdict = verdict_for(payload)
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_SCENARIO_FAILED, verdict.message)
        return verdict

    def test_good_report_passes(self) -> None:
        verdict = verdict_for(platform_payload())
        self.assertEqual(verdict.exit_code, smoke_wait.EXIT_OK, verdict.message)

    def test_empty_thermal_status_fails(self) -> None:
        verdict = self.failing_verdict(lambda report: report["thermal"].update(status=""))
        self.assertIn("пустой thermal.status", verdict.message)

    def test_unregistered_thermal_listener_fails(self) -> None:
        verdict = self.failing_verdict(lambda report: report["thermal"].update(listener="failed: нет сервиса"))
        self.assertIn("thermal.listener", verdict.message)

    def test_absent_vibrator_fails(self) -> None:
        verdict = self.failing_verdict(lambda report: report["haptics"].update(vibrator="absent"))
        self.assertIn("haptics.vibrator", verdict.message)

    def test_unsupported_pattern_fails_and_is_named(self) -> None:
        verdict = self.failing_verdict(lambda report: report["haptics"]["patterns"][1].update(realization="unsupported"))
        self.assertIn("shutter", verdict.message)

    def test_frame_rate_not_applied_fails(self) -> None:
        verdict = self.failing_verdict(lambda report: report["frame_rate"].update(result="no_surface"))
        self.assertIn("frame_rate.result", verdict.message)


if __name__ == "__main__":
    unittest.main()
