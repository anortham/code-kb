import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import scripts.edit_snapshot as edit_snapshot
from scripts.edit_snapshot import SnapshotError, apply_bundle, capture_bundle, snapshot_tree


class EditSnapshotTests(unittest.TestCase):
    def test_snapshot_is_stable_and_includes_empty_directories(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "project"
            (root / "empty").mkdir(parents=True)
            (root / "source.txt").write_text("source", encoding="utf-8")

            first = snapshot_tree(root)
            second = snapshot_tree(root)

            self.assertEqual(first, second)
            self.assertEqual(first["schema"], 1)
            self.assertEqual([entry["path"] for entry in first["entries"]], ["empty", "source.txt"])
            self.assertEqual(len(first["digest"]), 64)

    def test_capture_and_apply_round_trip_git_invisible_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            (seed / "tree" / "empty").mkdir(parents=True)
            (seed / ".gitignore").write_text("ignored.txt\n", encoding="utf-8")
            (seed / "keep.txt").write_text("before", encoding="utf-8")
            (seed / "mode-only.txt").write_text("same", encoding="utf-8")
            (seed / "remove.txt").write_text("remove", encoding="utf-8")
            (seed / "tree" / "source.txt").write_text("source", encoding="utf-8")
            (seed / "gone-link").symlink_to("tree/source.txt")
            seed_manifest = snapshot_tree(seed)
            self._copy_tree(seed, trial)

            (trial / "keep.txt").write_text("after", encoding="utf-8")
            (trial / "keep.txt").chmod(0o755)
            (trial / "mode-only.txt").chmod(0o755)
            (trial / "remove.txt").unlink()
            (trial / "gone-link").unlink()
            (trial / "tree" / "empty").rmdir()
            (trial / "ignored.txt").write_text("ignored", encoding="utf-8")
            (trial / "new" / "empty").mkdir(parents=True)
            (trial / "new" / "current").symlink_to("../tree/source.txt")

            output = base / "bundle"
            bundle = capture_bundle(seed_manifest, trial, output)
            replay = apply_bundle(seed, output, base / "replay")

            self.assertEqual(bundle["seed_digest"], seed_manifest["digest"])
            self.assertEqual(bundle["final_digest"], snapshot_tree(trial)["digest"])
            self.assertEqual(bundle["violations"], [])
            self.assertEqual(replay, snapshot_tree(trial))
            self.assertTrue((base / "replay" / "mode-only.txt").stat().st_mode & 0o111)

    def test_tampered_blob_is_rejected_before_replay(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "new.txt").write_text("new payload", encoding="utf-8")
            output = base / "bundle"
            capture_bundle(snapshot_tree(seed), trial, output)
            blob = next((output / "blobs").iterdir())
            blob.write_bytes(b"tampered")
            destination = base / "replay"

            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, destination)

            self.assertFalse(destination.exists())
            self.assertTrue((output / "bundle.json").is_file())

    def test_blob_symlink_is_rejected_without_reading_its_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "new.txt").write_text("new payload", encoding="utf-8")
            output = base / "bundle"
            capture_bundle(snapshot_tree(seed), trial, output)
            blob = next((output / "blobs").iterdir())
            outside = base / "outside"
            outside.write_text(blob.read_text(encoding="utf-8"), encoding="utf-8")
            blob.unlink()
            blob.symlink_to(outside)
            destination = base / "replay"

            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, destination)

            self.assertFalse(destination.exists())
            self.assertTrue(outside.is_file())

    def test_blob_directory_swap_to_symlink_is_rejected_before_replay(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "new.txt").write_text("new payload", encoding="utf-8")
            output = base / "bundle"
            capture_bundle(snapshot_tree(seed), trial, output)
            blob_root = output / "blobs"
            external = base / "external-blobs"
            destination = base / "replay"
            read_bundle = edit_snapshot._read_bundle

            def read_then_swap_blobs(bundle_path):
                data = read_bundle(bundle_path)
                blob_root.rename(external)
                blob_root.symlink_to(external, target_is_directory=True)
                return data

            with patch("scripts.edit_snapshot._read_bundle", side_effect=read_then_swap_blobs):
                with self.assertRaises(SnapshotError):
                    apply_bundle(seed, output, destination)

            self.assertFalse(destination.exists())

    def test_symlink_cycles_are_retained_as_invalid_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "first").symlink_to("second")
            (trial / "second").symlink_to("first")
            output = base / "bundle"

            bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(bundle["violations"])
            self.assertTrue((output / "bundle.json").is_file())
            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, base / "replay")
            with self.assertRaises(SnapshotError):
                snapshot_tree(trial)

    def test_long_symlink_chain_is_retained_as_invalid_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            chain_size = 1200
            for index in range(chain_size):
                target = f"chain-{index + 1:03d}" if index + 1 < chain_size else "missing"
                (trial / f"chain-{index:03d}").symlink_to(target)
            output = base / "bundle"

            bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(any(item["code"] == "symlink_depth_limit" for item in bundle["violations"]))
            self.assertTrue((output / "bundle.json").is_file())
            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, base / "replay")
            with self.assertRaises(SnapshotError):
                snapshot_tree(trial)

    def test_traversal_change_is_rejected_before_replay(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "new.txt").write_text("new payload", encoding="utf-8")
            output = base / "bundle"
            capture_bundle(snapshot_tree(seed), trial, output)
            bundle_path = output / "bundle.json"
            bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
            bundle["changes"][0]["path"] = "../outside"
            bundle_path.write_text(json.dumps(bundle), encoding="utf-8")
            destination = base / "replay"

            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, destination)

            self.assertFalse(destination.exists())
            self.assertFalse((base / "outside").exists())

    def test_escaping_link_is_retained_as_invalid_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            self._copy_tree(seed, trial)
            (trial / "escape").symlink_to("../../outside")
            output = base / "bundle"

            bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(bundle["violations"])
            self.assertTrue((output / "bundle.json").is_file())
            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, base / "replay")
            with self.assertRaises(SnapshotError):
                snapshot_tree(trial)

    def test_special_file_is_retained_as_invalid_evidence(self):
        if not hasattr(os, "mkfifo"):
            self.skipTest("named pipes are unavailable")
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            self._copy_tree(seed, trial)
            os.mkfifo(trial / "pipe")
            output = base / "bundle"

            bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(bundle["violations"])
            self.assertTrue((output / "bundle.json").is_file())

    def test_payload_limit_is_retained_as_invalid_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "payload.bin").write_bytes(b"123")
            output = base / "bundle"

            bundle = capture_bundle(snapshot_tree(seed), trial, output, max_bytes=2)

            self.assertTrue(bundle["violations"])
            self.assertTrue((output / "bundle.json").is_file())
            self.assertEqual(list((output / "blobs").iterdir()), [])

    def test_capture_retains_oversized_file_without_hashing_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "large.bin").write_bytes(b"12345")
            output = base / "bundle"

            with patch("scripts.edit_snapshot.MAX_FILE_BYTES", 4, create=True), patch.object(edit_snapshot, "_hash_regular_file", side_effect=AssertionError("oversized input was hashed")):
                bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(bundle["violations"])
            self.assertTrue((output / "bundle.json").is_file())

    def test_snapshot_rejects_oversized_seed_file_before_hashing_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "large.bin").write_bytes(b"12345")

            with patch("scripts.edit_snapshot.MAX_FILE_BYTES", 4, create=True), patch.object(edit_snapshot, "_hash_regular_file", side_effect=AssertionError("oversized input was hashed")) as hash_file:
                with self.assertRaises(SnapshotError):
                    snapshot_tree(root)

            hash_file.assert_not_called()

    def test_capture_stops_hashing_at_the_tree_byte_limit(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "first.bin").write_bytes(b"12")
            (trial / "second.bin").write_bytes(b"34")
            output = base / "bundle"
            original_hash = edit_snapshot._hash_regular_file

            with patch("scripts.edit_snapshot.MAX_SCAN_BYTES", 3, create=True), patch.object(edit_snapshot, "_hash_regular_file", wraps=original_hash) as hash_file:
                bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(any(item["code"] == "scan_size_limit" for item in bundle["violations"]))
            self.assertEqual(hash_file.call_count, 1)
            self.assertTrue((output / "bundle.json").is_file())

    def test_entry_limit_is_retained_as_invalid_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            trial.mkdir()
            (trial / "one").write_text("1", encoding="utf-8")
            (trial / "two").write_text("2", encoding="utf-8")
            output = base / "bundle"

            with patch("scripts.edit_snapshot.MAX_ENTRIES", 1):
                bundle = capture_bundle(snapshot_tree(seed), trial, output)

            self.assertTrue(bundle["violations"])
            self.assertTrue((output / "bundle.json").is_file())

    def test_protected_changes_are_retained_as_invalid_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            seed.mkdir()
            (seed / "tests").mkdir()
            (seed / "tests" / "canonical.py").write_text("before", encoding="utf-8")
            self._copy_tree(seed, trial)
            (trial / "tests" / "canonical.py").write_text("after", encoding="utf-8")
            output = base / "bundle"

            bundle = capture_bundle(snapshot_tree(seed), trial, output, protected=("tests",))

            self.assertTrue(bundle["violations"])
            with self.assertRaises(SnapshotError):
                apply_bundle(seed, output, base / "replay", protected=("tests",))

    def test_wrong_seed_is_rejected_before_replay(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            seed = base / "seed"
            trial = base / "trial"
            wrong_seed = base / "wrong-seed"
            seed.mkdir()
            trial.mkdir()
            wrong_seed.mkdir()
            (seed / "base.txt").write_text("seed", encoding="utf-8")
            (wrong_seed / "base.txt").write_text("other", encoding="utf-8")
            (trial / "new.txt").write_text("new", encoding="utf-8")
            output = base / "bundle"
            capture_bundle(snapshot_tree(seed), trial, output)
            destination = base / "replay"

            with self.assertRaises(SnapshotError):
                apply_bundle(wrong_seed, output, destination)

            self.assertFalse(destination.exists())

    def test_protected_excluded_overlap_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "tests").mkdir()

            with self.assertRaises(ValueError):
                capture_bundle(snapshot_tree(root), root, root.parent / "bundle", protected=("tests",), excluded=("tests/generated",))

    @staticmethod
    def _copy_tree(source, destination):
        shutil.copytree(source, destination, symlinks=True)


if __name__ == "__main__":
    unittest.main()
