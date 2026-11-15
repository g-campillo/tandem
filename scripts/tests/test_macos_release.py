import importlib.util
import json
import shutil
import sqlite3
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('release',Path(__file__).parents[1]/'macos_release.py')
release=importlib.util.module_from_spec(spec);spec.loader.exec_module(release)

class ReleaseSafety(unittest.TestCase):
    def fixture(self,root,name):
        bundle=root/name/'Tandem.app';resources=bundle/'Contents/Resources';resources.mkdir(parents=True)
        executable=bundle/'Contents/MacOS/tandem';executable.parent.mkdir();executable.write_bytes(b'test executable')
        files={}
        for relative in release.RUNTIME_HASHES:
            file=resources/'codex'/relative;file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(relative.encode());files[str(file.relative_to(bundle))]=release.digest(file)
        (resources/'notice.txt').write_text('Fixture notice');files['Contents/Resources/notice.txt']=release.digest(resources/'notice.txt')
        manifest={'schema':1,'product':'Tandem','release_id':name,'files':files}
        release.write_json(resources/'release-manifest.json',manifest)
        return bundle
    def test_install_and_rollback_preserve_versions_and_reject_unrelated_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);a=self.fixture(root,'one');b=self.fixture(root,'two');hashes={r:release.digest(a/'Contents/Resources/codex'/r) for r in release.RUNTIME_HASHES}
            with patch.object(release,'RUNTIME_HASHES',hashes),patch.object(release,'verify_signature'):
                apps=root/'Applications';first=release.install(a,apps);second=release.install(b,apps)
                self.assertEqual((apps/'Tandem.app').resolve(),Path(second['current']))
                self.assertTrue(Path(first['current']).exists())
                self.assertEqual(release.install(b,apps),second)
                release.rollback(apps);self.assertEqual((apps/'Tandem.app').resolve(),Path(first['current']))
                unrelated=root/'Unrelated';unrelated.mkdir();(unrelated/'Tandem.app').mkdir();(unrelated/'Tandem.app/user-work').write_text('keep')
                with self.assertRaises(ValueError):release.install(a,unrelated)
                self.assertEqual((unrelated/'Tandem.app/user-work').read_text(),'keep')
    def test_packaged_schema_five_profile_survives_older_release_rollback(self):
        # This simulates the older app's schema guard, not an installer gate.
        # Real Store refusal is covered by the tandem-core workspace tests.
        def simulate_app_open(bundle, profile):
            manifest = release.verify_bundle(bundle)
            with sqlite3.connect(profile.as_uri() + '?mode=ro', uri=True) as connection:
                version = connection.execute('PRAGMA user_version').fetchone()[0]
            if version > manifest['database_schema']:
                raise ValueError('Profile belongs to a newer Tandem version')

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            older = self.fixture(root, 'older-schema-four')
            manifest_path = older / 'Contents/Resources/release-manifest.json'
            manifest = json.loads(manifest_path.read_text())
            manifest['database_schema'] = 4
            release.write_json(manifest_path, manifest)
            hashes = {r: release.digest(older / 'Contents/Resources/codex' / r)
                      for r in release.RUNTIME_HASHES}
            binary = root / 'tandem'
            binary.write_bytes(b'current app fixture')
            notices = root / 'ThirdPartyNotices'
            notices.mkdir()
            release.write_json(notices / 'inventory.json', {
                'tandem_lock_sha256': release.digest(release.ROOT / 'Cargo.lock'),
                'unresolved': [],
            })
            profile = root / 'workspace.sqlite'
            with sqlite3.connect(profile) as connection:
                connection.executescript("CREATE TABLE session(value TEXT); INSERT INTO session VALUES('preserve conversation'); PRAGMA user_version=5;")
            original = profile.read_bytes()
            apps = root / 'Applications'
            current = root / 'current-schema-five/Tandem.app'

            def copy_fixture_runtime(resources):
                shutil.copytree(older / 'Contents/Resources/codex', resources / 'codex')

            with patch.object(release, 'RUNTIME_HASHES', hashes), \
                 patch.object(release, 'copy_runtime', side_effect=copy_fixture_runtime), \
                 patch.object(release, 'verify_signature'), \
                 patch.object(release.subprocess, 'run'):
                release.package(binary, notices, current, 'current-schema-five')
                release.install(older, apps)
                installed = release.install(current, apps)
                simulate_app_open(Path(installed['current']), profile)
                self.assertEqual(profile.read_bytes(), original)
                release.rollback(apps)
                selected = (apps / 'Tandem.app').resolve()
                self.assertEqual(selected.name, 'Tandem.app')
                self.assertEqual(selected.parent.name, 'older-schema-four')
                with self.assertRaisesRegex(ValueError, 'newer Tandem version'):
                    simulate_app_open(selected, profile)
                self.assertEqual(profile.read_bytes(), original)
                self.assertTrue(Path(installed['current']).exists())

    def test_concurrent_installation_is_refused_without_switching_the_link(self):
        with tempfile.TemporaryDirectory() as tmp:
            apps=Path(tmp)/'Applications'
            with release.installation_lock(apps):
                with self.assertRaises(BlockingIOError):
                    with release.installation_lock(apps):pass
            self.assertFalse((apps/'Tandem.app').exists())

    def test_corruption_extra_files_and_escaping_manifest_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle=self.fixture(root,'one');hashes={r:release.digest(bundle/'Contents/Resources/codex'/r) for r in release.RUNTIME_HASHES}
            with patch.object(release,'RUNTIME_HASHES',hashes),patch.object(release,'verify_signature'):
                release.verify_bundle(bundle)
                notice=bundle/'Contents/Resources/notice.txt';notice.write_text('tampered')
                with self.assertRaises(ValueError):release.verify_bundle(bundle)
                notice.write_text('Fixture notice');extra=bundle/'extra';extra.write_text('unexpected')
                with self.assertRaises(ValueError):release.verify_bundle(bundle)
                extra.unlink();manifest=bundle/'Contents/Resources/release-manifest.json';value=json.loads(manifest.read_text());value['files']['../../outside']='0'*64;release.write_json(manifest,value)
                with self.assertRaises(ValueError):release.verify_bundle(bundle)
    def test_release_ids_and_symlink_destinations_cannot_escape_managed_storage(self):
        for value in ['../release','/tmp/release','name/child','.', '..']:
            with self.assertRaises(ValueError):release.release_name(value)
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);apps=root/'Applications';apps.mkdir();outside=root/'Other.app';outside.mkdir();(apps/'Tandem.app').symlink_to(outside)
            with self.assertRaises(ValueError):release.check_link(apps/'Tandem.app',apps/'Tandem Releases')
            self.assertEqual((apps/'Tandem.app').resolve(),outside.resolve())
if __name__=='__main__':unittest.main()
