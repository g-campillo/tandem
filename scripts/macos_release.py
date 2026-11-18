"""Build, verify and install immutable local Tandem releases (macOS, Python 3)."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import plistlib
import re
import shutil
import subprocess
import tempfile
import uuid
import fcntl
from contextlib import contextmanager

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / 'crates/tandem-claude/helper'
PINS = json.loads((ROOT / 'scripts/release-inputs.json').read_text())
CLAUDE_BINARY = 'node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude'
SDK_LOCK_KEY = 'node_modules/@anthropic-ai/claude-agent-sdk'
APP_ICON = ROOT / 'crates/tandem/resources/Tandem.icon'
MINIMUM_MACOS = '27.0'
# Listing every file of the helper's npm tree makes a manifest of about 1.5 MB.
MANIFEST_LIMIT = 8 * 1024 * 1024

def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()

def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as out:
        temporary = Path(out.name)
        out.write((json.dumps(value, indent=2) + '\n').encode())
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, path)

def release_name(value):
    if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9._-]{0,79}', value):
        raise ValueError('Release id must be a short filename, not a path')
    return value

def check_helper_pins():
    """Refuse a helper install whose Claude Agent SDK or Claude Code binary is not the pinned one."""
    if not (HELPER / 'node_modules').is_dir():
        raise ValueError('Run npm ci in crates/tandem-claude/helper first')
    lock = json.loads((HELPER / 'package-lock.json').read_text())
    version = lock.get('packages', {}).get(SDK_LOCK_KEY, {}).get('version')
    if version != PINS['claude_agent_sdk_version']:
        raise ValueError(f"Pinned Claude Agent SDK changed: the helper lockfile has {version}, "
                         f"scripts/release-inputs.json pins {PINS['claude_agent_sdk_version']}")
    binary = HELPER / CLAUDE_BINARY
    if not binary.is_file() or digest(binary) != PINS['claude_code_binary_sha256']:
        raise ValueError('Pinned Claude Agent SDK changed: ' + CLAUDE_BINARY
                         + ' does not match the fingerprint in scripts/release-inputs.json')

def _skip_installer_files(directory, names):
    # npm's .bin folders hold symlinks to package scripts, and the hidden
    # lockfile is npm's install cache; neither is needed to run the helper.
    skipped = {name for name in names if name == '.bin' and (Path(directory) / name).is_dir()}
    if Path(directory) == HELPER / 'node_modules' and '.package-lock.json' in names:
        skipped.add('.package-lock.json')
    return skipped

def copy_helper(resources):
    """Copy the installed Claude helper (no tests, no npm links) into the bundle resources."""
    check_helper_pins()
    target = resources / 'claude-helper'
    if target.is_symlink() or target.is_file():
        target.unlink()
    elif target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    for name in ['main.mjs', 'package.json', 'package-lock.json']:
        shutil.copy2(HELPER / name, target / name, follow_symlinks=False)
    shutil.copytree(HELPER / 'src', target / 'src', symlinks=True)
    shutil.copytree(HELPER / 'node_modules', target / 'node_modules', symlinks=True, ignore=_skip_installer_files)
    links = sorted(path for path in target.rglob('*') if path.is_symlink())
    if links:
        shutil.rmtree(target)
        raise ValueError('The helper contains a symlink: ' + str(links[0].relative_to(target)))

def compile_icon(resources):
    """Compile the Liquid Glass app icon into resources; returns the Info.plist keys that name it.

    actool writes Assets.car (light, dark and tinted renditions) and a Tandem.icns
    fallback. Needs Xcode: the Command Line Tools do not include actool.
    """
    with tempfile.TemporaryDirectory(prefix='.tandem-icon-') as temporary:
        output = Path(temporary)
        result = subprocess.run(['/usr/bin/xcrun', 'actool', str(APP_ICON), '--compile', str(output),
                                 '--platform', 'macosx', '--minimum-deployment-target', MINIMUM_MACOS,
                                 '--app-icon', APP_ICON.stem, '--output-format', 'xml1',
                                 '--output-partial-info-plist', str(output / 'partial.plist')],
                                capture_output=True)
        try:
            errors = plistlib.loads(result.stdout).get('com.apple.actool.errors', [])
        except Exception:
            errors = [{'description': result.stderr.decode(errors='replace').strip() or 'actool failed'}]
        if result.returncode or errors:
            reason = '; '.join(error.get('description', '') for error in errors) or f'actool exited with {result.returncode}'
            raise ValueError('The app icon did not compile: ' + reason)
        for name in ['Assets.car', APP_ICON.stem + '.icns']:
            if not (output / name).is_file():
                raise ValueError('The app icon did not compile: actool wrote no ' + name)
            shutil.copy2(output / name, resources / name)
        return plistlib.loads((output / 'partial.plist').read_bytes())

def verify_signature(bundle):
    subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(bundle)], check=True, capture_output=True)

def verify_bundle(bundle):
    if bundle.is_symlink():
        raise ValueError('Verify the concrete versioned bundle, not a symlink')
    manifest_path = bundle / 'Contents/Resources/release-manifest.json'
    if manifest_path.stat().st_size > MANIFEST_LIMIT:
        raise ValueError('Oversized release manifest')
    manifest = json.loads(manifest_path.read_text())
    release_name(manifest['release_id'])
    if manifest.get('product') != 'Tandem' or manifest.get('schema') != 1:
        raise ValueError('This is not a supported Tandem release')
    for relative, expected in manifest['files'].items():
        path = Path(relative)
        if path.is_absolute() or '..' in path.parts:
            raise ValueError('Manifest path escaped the bundle')
        file = bundle / path
        if any((bundle / Path(*path.parts[:i])).is_symlink() for i in range(1,len(path.parts)+1)):
            raise ValueError('Release files cannot contain symlinks')
        if digest(file) != expected:
            raise ValueError('Release file changed: ' + relative)
    actual = {str(p.relative_to(bundle)) for p in bundle.rglob('*') if p.is_file()
              and '_CodeSignature' not in p.parts and p != manifest_path and p != bundle/'Contents/MacOS/tandem'}
    if actual != set(manifest['files']):
        raise ValueError('Release inventory has missing or unexpected files')
    # Releases from before the Claude helper carry no helper to check; their
    # files are still verified above, so install and rollback keep working.
    if 'claude_agent_sdk_version' in manifest:
        helper = bundle / 'Contents/Resources/claude-helper'
        if not (helper / 'main.mjs').is_file():
            raise ValueError('The bundled Claude helper is missing')
        if not (helper / CLAUDE_BINARY).is_file() or digest(helper / CLAUDE_BINARY) != PINS['claude_code_binary_sha256']:
            raise ValueError('Claude Code fingerprint changed')
    executable=bundle/'Contents/MacOS/tandem'
    if executable.is_symlink() or not executable.is_file():
        raise ValueError('Missing app executable')
    verify_signature(bundle)
    return manifest

def package(binary, notices, destination, release_id):
    release_name(release_id)
    if destination.exists() or destination.is_symlink():
        raise ValueError('Release destination exists; choose a new release id')
    inventory = json.loads((notices / 'inventory.json').read_text())
    if inventory.get('tandem_lock_sha256') != digest(ROOT/'Cargo.lock'):
        raise ValueError('Notice inventory does not match the current Cargo.lock; regenerate it')
    if inventory.get('helper_lock_sha256') != digest(HELPER / 'package-lock.json'):
        raise ValueError('Notice inventory does not match the helper lockfile; regenerate it')
    if inventory.get('unresolved'):
        raise ValueError('Resolve missing third-party notices before packaging')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.tandem-package-', dir=destination.parent) as temporary:
        bundle = Path(temporary) / 'Tandem.app'
        macos = bundle / 'Contents/MacOS'
        resources = bundle / 'Contents/Resources'
        macos.mkdir(parents=True)
        resources.mkdir()
        shutil.copy2(binary, macos / 'tandem')
        (macos / 'tandem').chmod(0o755)
        copy_helper(resources)
        shutil.copytree(notices, resources / 'ThirdPartyNotices')
        (resources / 'LOCAL-USE.md').write_text((ROOT / 'docs/tandem/local-use.md').read_text())
        icon_keys = compile_icon(resources)
        (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleExecutable': 'tandem', 'CFBundleIdentifier': 'dev.tandem.ide',
            'CFBundleName': 'Tandem', 'CFBundlePackageType': 'APPL',
            'CFBundleShortVersionString': '0.1.0', 'CFBundleVersion': '6',
            'LSMinimumSystemVersion': MINIMUM_MACOS, 'NSHighResolutionCapable': True, **icon_keys,
        }))
        # Sign the main executable before hashing it; the outer seal does not
        # re-sign the bundled Claude Code binary and so preserves its pinned bytes.
        subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(macos / 'tandem')], check=True, capture_output=True)
        files = {str(p.relative_to(bundle)): digest(p) for p in sorted(bundle.rglob('*')) if p.is_file() and p != macos/'tandem' and '_CodeSignature' not in p.parts}
        write_json(resources / 'release-manifest.json', {
            'schema': 1, 'product': 'Tandem', 'release_id': release_id,
            'target': 'aarch64-apple-darwin', 'app_version': '0.1.0',
            'claude_agent_sdk_version': PINS['claude_agent_sdk_version'],
            'claude_code_version': PINS['claude_code_version'], 'database_schema': 6,
            'signature': 'ad-hoc', 'notarized': False, 'files': files,
            'source_binary_sha256':digest(binary),
            'executable_integrity':'verified by the code signature; excluded from the resource manifest to avoid a circular seal',
        })
        subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(bundle)], check=True, capture_output=True)
        verify_bundle(bundle)
        os.rename(bundle, destination)
    return destination

def installation_paths(applications):
    applications = applications.expanduser().absolute()
    applications.mkdir(parents=True, exist_ok=True)
    applications=applications.resolve()
    return applications, applications / 'Tandem Releases', applications / 'Tandem.app'

def check_link(link, releases):
    if not link.exists() and not link.is_symlink():
        return None
    if not link.is_symlink():
        raise ValueError('Existing Tandem.app is not a managed release link; it was not changed')
    current = link.resolve(strict=True)
    if not current.is_relative_to(releases.resolve()) or current.name != 'Tandem.app':
        raise ValueError('Existing app link is outside managed releases; it was not changed')
    verify_bundle(current)
    return current

def activate(link, bundle):
    # Atomic replacement of only the launcher link; a running versioned bundle
    # is never overwritten. Its process may keep running until the user quits.
    temporary = link.parent / ('.tandem-link-' + uuid.uuid4().hex)
    temporary.symlink_to(os.path.relpath(bundle, link.parent))
    os.replace(temporary, link)

def _install(bundle, applications):
    manifest = verify_bundle(bundle)
    applications, releases, link = installation_paths(applications)
    if releases.is_symlink():raise ValueError('Managed release directory cannot be a symlink')
    releases.mkdir(exist_ok=True)
    previous = check_link(link, releases)
    target = releases / manifest['release_id'] / 'Tandem.app'
    if target.exists():
        if verify_bundle(target) != manifest:
            raise ValueError('A different bundle already uses this release id')
    else:
        with tempfile.TemporaryDirectory(prefix='.install-', dir=releases) as temporary:
            pending = Path(temporary) / 'Tandem.app'
            shutil.copytree(bundle, pending)
            verify_bundle(pending)
            target.parent.mkdir(exist_ok=False)
            os.rename(pending, target)
    if previous == target.resolve() and (releases/'installation.json').exists():
        saved=json.loads((releases/'installation.json').read_text())
        if saved.get('current')==str(target):return saved
    activate(link, target)
    receipt = {'schema': 1, 'current': str(target), 'previous': str(previous) if previous else None,
               'release_id': manifest['release_id'], 'state_preserved': True}
    write_json(releases / 'installation.json', receipt)
    return receipt

def _rollback(applications):
    _, releases, link = installation_paths(applications)
    current = check_link(link, releases)
    receipt = json.loads((releases / 'installation.json').read_text())
    if not current or str(current) != receipt['current'] or not receipt.get('previous'):
        raise ValueError('No matching previous managed release is available')
    previous = Path(receipt['previous'])
    if not previous.resolve().is_relative_to(releases.resolve()):
        raise ValueError('Previous release escaped managed storage')
    manifest = verify_bundle(previous)
    activate(link, previous)
    write_json(releases / 'installation.json', {'schema': 1, 'current': str(previous),
               'previous': str(current), 'release_id': manifest['release_id'], 'state_preserved': True})
    return previous

@contextmanager
def installation_lock(applications):
    _,releases,_=installation_paths(applications)
    if releases.is_symlink():raise ValueError('Managed release directory cannot be a symlink')
    releases.mkdir(exist_ok=True)
    descriptor=os.open(releases/'.installation.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
    try:
        fcntl.flock(descriptor,fcntl.LOCK_EX|fcntl.LOCK_NB)
        yield
    finally:
        os.close(descriptor)

def install(bundle,applications):
    with installation_lock(applications):return _install(bundle,applications)

def rollback(applications):
    with installation_lock(applications):return _rollback(applications)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    build = commands.add_parser('package')
    build.add_argument('--binary', type=Path, default=ROOT/'target/release/tandem')
    build.add_argument('--notices', type=Path, required=True)
    build.add_argument('--release-id', required=True)
    build.add_argument('--output', type=Path)
    for name in ['verify','install']:
        command=commands.add_parser(name);command.add_argument('bundle',type=Path)
        if name=='install':command.add_argument('--applications',type=Path,default=Path.home()/'Applications')
    command=commands.add_parser('rollback');command.add_argument('--applications',type=Path,default=Path.home()/'Applications')
    args=parser.parse_args()
    try:
        if args.command=='package':print(package(args.binary,args.notices,args.output or ROOT/'.tandem-local/releases'/release_name(args.release_id)/'Tandem.app',args.release_id))
        elif args.command=='verify':print(json.dumps(verify_bundle(args.bundle),indent=2))
        elif args.command=='install':print(json.dumps(install(args.bundle,args.applications),indent=2))
        else:print(rollback(args.applications))
    except (ValueError,OSError,subprocess.CalledProcessError) as error:
        parser.exit(1,f'{error}\n')
if __name__=='__main__':main()
