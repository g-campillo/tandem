import importlib.util
import json
import plistlib
import subprocess
import shutil
import sqlite3
from contextlib import closing
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('release',Path(__file__).parents[1]/'macos_release.py')
release=importlib.util.module_from_spec(spec);spec.loader.exec_module(release)

FIXTURE_BINARY=b'fixture Claude Code binary'
SDK='node_modules/@anthropic-ai/claude-agent-sdk'

def pinned(binary):
    return {**release.PINS,'claude_code_binary_sha256':release.digest(binary)}

def fake_helper(root,sdk_version=None):
    """A small installed helper tree laid out like `npm ci` leaves it."""
    helper=root/'helper';helper.mkdir()
    (helper/'main.mjs').write_text('// entry point\n')
    (helper/'package.json').write_text('{"name":"tandem-claude-helper"}\n')
    lock={'lockfileVersion':3,'packages':{'':{'name':'tandem-claude-helper'},
          SDK:{'version':sdk_version or release.PINS['claude_agent_sdk_version']}}}
    (helper/'package-lock.json').write_text(json.dumps(lock))
    (helper/'src').mkdir();(helper/'src/session.mjs').write_text('export {}\n')
    (helper/'test').mkdir();(helper/'test/session.test.mjs').write_text('// test\n')
    modules=helper/'node_modules';(modules/'pkg/node_modules/.bin').mkdir(parents=True)
    (modules/'pkg/index.js').write_text('module.exports = 1\n')
    (modules/'pkg/node_modules/.bin/nested').write_text('#!/bin/sh\n')
    (modules/'.bin').mkdir();(modules/'.bin/pkg').symlink_to('../pkg/index.js')
    (modules/'.package-lock.json').write_text('{}\n')
    (modules/SDK.removeprefix('node_modules/')).mkdir(parents=True)
    (modules/'@anthropic-ai/claude-agent-sdk/sdk.mjs').write_text('export {}\n')
    binary=helper/release.CLAUDE_BINARY;binary.parent.mkdir(parents=True);binary.write_bytes(FIXTURE_BINARY)
    return helper

class ReleaseSafety(unittest.TestCase):
    def fixture(self,root,name,helper=True):
        """A signed-release lookalike; helper=False is a release from before the Claude helper."""
        bundle=root/name/'Tandem.app';resources=bundle/'Contents/Resources';resources.mkdir(parents=True)
        executable=bundle/'Contents/MacOS/tandem';executable.parent.mkdir();executable.write_bytes(b'test executable')
        files={}
        contents=[('claude-helper/main.mjs',b'// entry point\n'),('claude-helper/'+release.CLAUDE_BINARY,FIXTURE_BINARY)] if helper else [('runtime/bin/agent',b'older agent runtime')]
        for relative,content in contents:
            file=resources/relative;file.parent.mkdir(parents=True,exist_ok=True);file.write_bytes(content);files[str(file.relative_to(bundle))]=release.digest(file)
        (resources/'notice.txt').write_text('Fixture notice');files['Contents/Resources/notice.txt']=release.digest(resources/'notice.txt')
        manifest={'schema':1,'product':'Tandem','release_id':name,'files':files}
        if helper:manifest['claude_agent_sdk_version']=release.PINS['claude_agent_sdk_version']
        release.write_json(resources/'release-manifest.json',manifest)
        return bundle
    def bundled_binary(self,bundle):
        return bundle/'Contents/Resources/claude-helper'/release.CLAUDE_BINARY
    def test_install_and_rollback_preserve_versions_and_reject_unrelated_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);a=self.fixture(root,'one');b=self.fixture(root,'two')
            with patch.object(release,'PINS',pinned(self.bundled_binary(a))),patch.object(release,'verify_signature'):
                apps=root/'Applications';first=release.install(a,apps);second=release.install(b,apps)
                self.assertEqual((apps/'Tandem.app').resolve(),Path(second['current']))
                self.assertTrue(Path(first['current']).exists())
                self.assertEqual(release.install(b,apps),second)
                release.rollback(apps);self.assertEqual((apps/'Tandem.app').resolve(),Path(first['current']))
                unrelated=root/'Unrelated';unrelated.mkdir();(unrelated/'Tandem.app').mkdir();(unrelated/'Tandem.app/user-work').write_text('keep')
                with self.assertRaises(ValueError):release.install(a,unrelated)
                self.assertEqual((unrelated/'Tandem.app/user-work').read_text(),'keep')
    def test_install_over_and_rollback_to_a_release_from_before_the_claude_helper(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);older=self.fixture(root,'older',helper=False);current=self.fixture(root,'current')
            with patch.object(release,'PINS',pinned(self.bundled_binary(current))),patch.object(release,'verify_signature'):
                apps=root/'Applications';release.install(older,apps);installed=release.install(current,apps)
                self.assertEqual((apps/'Tandem.app').resolve(),Path(installed['current']))
                release.rollback(apps);self.assertEqual((apps/'Tandem.app').resolve().parent.name,'older')
    def test_a_release_that_declares_the_claude_helper_must_contain_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle=self.fixture(root,'one');helper=bundle/'Contents/Resources/claude-helper'
            pins=pinned(self.bundled_binary(bundle));(helper/'main.mjs').unlink()
            manifest=bundle/'Contents/Resources/release-manifest.json';value=json.loads(manifest.read_text())
            del value['files']['Contents/Resources/claude-helper/main.mjs'];release.write_json(manifest,value)
            with patch.object(release,'PINS',pins),patch.object(release,'verify_signature'):
                with self.assertRaisesRegex(ValueError,'bundled Claude helper is missing'):release.verify_bundle(bundle)
    def test_packaged_schema_six_profile_survives_older_release_rollback(self):
        # This simulates the older app's schema guard, not an installer gate.
        # Real Store refusal is covered by the tandem-core workspace tests.
        def simulate_app_open(bundle, profile):
            manifest = release.verify_bundle(bundle)
            with closing(sqlite3.connect(profile.as_uri() + '?mode=ro', uri=True)) as connection:
                version = connection.execute('PRAGMA user_version').fetchone()[0]
            if version > manifest['database_schema']:
                raise ValueError('Profile belongs to a newer Tandem version')

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            older = self.fixture(root, 'older-schema-five')
            manifest_path = older / 'Contents/Resources/release-manifest.json'
            manifest = json.loads(manifest_path.read_text())
            manifest['database_schema'] = 5
            release.write_json(manifest_path, manifest)
            binary = root / 'tandem'
            binary.write_bytes(b'current app fixture')
            notices = root / 'ThirdPartyNotices'
            notices.mkdir()
            release.write_json(notices / 'inventory.json', {
                'tandem_lock_sha256': release.digest(release.ROOT / 'Cargo.lock'),
                'helper_lock_sha256': release.digest(release.HELPER / 'package-lock.json'),
                'unresolved': [],
            })
            profile = root / 'workspace.sqlite'
            with closing(sqlite3.connect(profile)) as connection:
                connection.executescript("CREATE TABLE session(value TEXT); INSERT INTO session VALUES('preserve conversation'); PRAGMA user_version=6;")
            original = profile.read_bytes()
            apps = root / 'Applications'
            current = root / 'current-schema-six/Tandem.app'

            def copy_fixture_helper(resources):
                shutil.copytree(older / 'Contents/Resources/claude-helper', resources / 'claude-helper')

            with patch.object(release, 'PINS', pinned(self.bundled_binary(older))), \
                 patch.object(release, 'copy_helper', side_effect=copy_fixture_helper), \
                 patch.object(release, 'compile_icon', return_value={'CFBundleIconName': 'Tandem'}), \
                 patch.object(release, 'verify_signature'), \
                 patch.object(release.subprocess, 'run'):
                release.package(binary, notices, current, 'current-schema-six')
                packaged = json.loads((current / 'Contents/Resources/release-manifest.json').read_text())
                self.assertEqual(plistlib.loads((current / 'Contents/Info.plist').read_bytes())['CFBundleIconName'], 'Tandem')
                self.assertEqual(packaged['database_schema'], 6)
                self.assertEqual(packaged['claude_agent_sdk_version'], release.PINS['claude_agent_sdk_version'])
                self.assertEqual(packaged['claude_code_version'], release.PINS['claude_code_version'])
                release.install(older, apps)
                installed = release.install(current, apps)
                simulate_app_open(Path(installed['current']), profile)
                self.assertEqual(profile.read_bytes(), original)
                release.rollback(apps)
                selected = (apps / 'Tandem.app').resolve()
                self.assertEqual(selected.name, 'Tandem.app')
                self.assertEqual(selected.parent.name, 'older-schema-five')
                with self.assertRaisesRegex(ValueError, 'newer Tandem version'):
                    simulate_app_open(selected, profile)
                self.assertEqual(profile.read_bytes(), original)
                self.assertTrue(Path(installed['current']).exists())

    def test_package_refuses_notices_collected_for_another_helper_lockfile(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);binary=root/'tandem';binary.write_bytes(b'app');notices=root/'ThirdPartyNotices';notices.mkdir()
            release.write_json(notices/'inventory.json',{'tandem_lock_sha256':release.digest(release.ROOT/'Cargo.lock'),'helper_lock_sha256':'0'*64,'unresolved':[]})
            with patch.object(release,'copy_helper') as copy:
                with self.assertRaisesRegex(ValueError,'helper lockfile'):release.package(binary,notices,root/'out/Tandem.app','out')
                copy.assert_not_called()
            self.assertFalse((root/'out').exists())

    @unittest.skipUnless(subprocess.run(['/usr/bin/xcrun', '--find', 'actool'], capture_output=True).returncode == 0, 'needs Xcode')
    def test_compile_icon_writes_the_asset_catalog_and_icns_and_names_them(self):
        with tempfile.TemporaryDirectory() as tmp:
            resources=Path(tmp)
            self.assertEqual(release.compile_icon(resources),{'CFBundleIconName':'Tandem','CFBundleIconFile':'Tandem'})
            self.assertEqual(sorted(p.name for p in resources.iterdir()),['Assets.car','Tandem.icns'])
    def test_compile_icon_reports_why_actool_failed(self):
        failure=subprocess.CompletedProcess([],1,stdout=plistlib.dumps({'com.apple.actool.errors':[{'description':'Failed to create a catalog'}]}),stderr=b'')
        with tempfile.TemporaryDirectory() as tmp, patch.object(release.subprocess,'run',return_value=failure):
            with self.assertRaisesRegex(ValueError,'did not compile: Failed to create a catalog'):release.compile_icon(Path(tmp))
            self.assertEqual(list(Path(tmp).iterdir()),[])
    def test_copy_helper_copies_the_runtime_without_installer_links_or_tests(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);helper=fake_helper(root);resources=root/'Resources'
            with patch.object(release,'HELPER',helper),patch.object(release,'PINS',pinned(helper/release.CLAUDE_BINARY)):
                release.copy_helper(resources)
            target=resources/'claude-helper'
            for relative in ['main.mjs','package.json','package-lock.json','src/session.mjs','node_modules/pkg/index.js','node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs',release.CLAUDE_BINARY]:
                self.assertTrue((target/relative).is_file(),relative)
            self.assertEqual((target/release.CLAUDE_BINARY).read_bytes(),FIXTURE_BINARY)
            self.assertFalse((target/'test').exists())
            self.assertFalse((target/'node_modules/.package-lock.json').exists())
            self.assertEqual([p for p in target.rglob('*') if p.name=='.bin'],[])
            self.assertEqual([p for p in target.rglob('*') if p.is_symlink()],[])

    def test_copy_helper_replaces_an_earlier_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);helper=fake_helper(root);resources=root/'Resources'
            stale=resources/'claude-helper/src/removed.mjs';stale.parent.mkdir(parents=True);stale.write_text('old')
            with patch.object(release,'HELPER',helper),patch.object(release,'PINS',pinned(helper/release.CLAUDE_BINARY)):
                release.copy_helper(resources)
            self.assertFalse(stale.exists())
            self.assertTrue((resources/'claude-helper/main.mjs').is_file())

    def test_copy_helper_requires_an_installed_helper(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);helper=fake_helper(root);shutil.rmtree(helper/'node_modules')
            with patch.object(release,'HELPER',helper):
                with self.assertRaisesRegex(ValueError,'Run npm ci in crates/tandem-claude/helper first'):release.copy_helper(root/'Resources')
            self.assertFalse((root/'Resources/claude-helper').exists())

    def test_copy_helper_refuses_a_claude_binary_that_differs_from_the_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);helper=fake_helper(root)
            with patch.object(release,'HELPER',helper),patch.object(release,'PINS',{**release.PINS,'claude_code_binary_sha256':'0'*64}):
                with self.assertRaisesRegex(ValueError,'Pinned Claude Agent SDK changed'):release.copy_helper(root/'Resources')
            self.assertFalse((root/'Resources/claude-helper').exists())

    def test_copy_helper_refuses_an_sdk_version_that_differs_from_the_pin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);helper=fake_helper(root,sdk_version='0.0.1')
            with patch.object(release,'HELPER',helper),patch.object(release,'PINS',pinned(helper/release.CLAUDE_BINARY)):
                with self.assertRaisesRegex(ValueError,'Pinned Claude Agent SDK changed: .*0.0.1'):release.copy_helper(root/'Resources')

    def test_copy_helper_refuses_a_symlink_outside_bin(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);helper=fake_helper(root);(helper/'node_modules/pkg/alias.js').symlink_to('index.js')
            with patch.object(release,'HELPER',helper),patch.object(release,'PINS',pinned(helper/release.CLAUDE_BINARY)):
                with self.assertRaisesRegex(ValueError,'The helper contains a symlink: node_modules/pkg/alias.js'):release.copy_helper(root/'Resources')
            self.assertFalse((root/'Resources/claude-helper').exists())

    def test_concurrent_installation_is_refused_without_switching_the_link(self):
        with tempfile.TemporaryDirectory() as tmp:
            apps=Path(tmp)/'Applications'
            with release.installation_lock(apps):
                with self.assertRaises(BlockingIOError):
                    with release.installation_lock(apps):pass
            self.assertFalse((apps/'Tandem.app').exists())

    def test_corruption_extra_files_and_escaping_manifest_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle=self.fixture(root,'one')
            with patch.object(release,'PINS',pinned(self.bundled_binary(bundle))),patch.object(release,'verify_signature'):
                release.verify_bundle(bundle)
                notice=bundle/'Contents/Resources/notice.txt';notice.write_text('tampered')
                with self.assertRaises(ValueError):release.verify_bundle(bundle)
                notice.write_text('Fixture notice');extra=bundle/'extra';extra.write_text('unexpected')
                with self.assertRaises(ValueError):release.verify_bundle(bundle)
                extra.unlink();manifest=bundle/'Contents/Resources/release-manifest.json';value=json.loads(manifest.read_text());value['files']['../../outside']='0'*64;release.write_json(manifest,value)
                with self.assertRaises(ValueError):release.verify_bundle(bundle)

    def test_verify_bundle_refuses_an_altered_claude_binary_even_with_a_matching_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle=self.fixture(root,'one');binary=self.bundled_binary(bundle)
            with patch.object(release,'PINS',pinned(binary)),patch.object(release,'verify_signature'):
                release.verify_bundle(bundle)
                binary.write_bytes(b'altered binary')
                manifest=bundle/'Contents/Resources/release-manifest.json';value=json.loads(manifest.read_text())
                value['files'][str(binary.relative_to(bundle))]=release.digest(binary);release.write_json(manifest,value)
                with self.assertRaisesRegex(ValueError,'Claude Code fingerprint changed'):release.verify_bundle(bundle)

    def test_manifest_limit_fits_a_bundled_helper_tree_but_stays_bounded(self):
        # Listing the helper's npm tree makes a real manifest about 1.5 MB.
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);bundle=self.fixture(root,'one');manifest=bundle/'Contents/Resources/release-manifest.json'
            value=json.loads(manifest.read_text())
            with patch.object(release,'PINS',pinned(self.bundled_binary(bundle))),patch.object(release,'verify_signature'):
                release.write_json(manifest,{**value,'padding':'x'*(2*1024*1024)});release.verify_bundle(bundle)
                release.write_json(manifest,{**value,'padding':'x'*(9*1024*1024)})
                with self.assertRaisesRegex(ValueError,'Oversized release manifest'):release.verify_bundle(bundle)
    def test_release_ids_and_symlink_destinations_cannot_escape_managed_storage(self):
        for value in ['../release','/tmp/release','name/child','.', '..']:
            with self.assertRaises(ValueError):release.release_name(value)
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);apps=root/'Applications';apps.mkdir();outside=root/'Other.app';outside.mkdir();(apps/'Tandem.app').symlink_to(outside)
            with self.assertRaises(ValueError):release.check_link(apps/'Tandem.app',apps/'Tandem Releases')
            self.assertEqual((apps/'Tandem.app').resolve(),outside.resolve())
if __name__=='__main__':unittest.main()
