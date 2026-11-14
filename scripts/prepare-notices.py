"""Prepare reproducible source metadata for the local notice collector; no build/install."""
from pathlib import Path
import hashlib,json,os,subprocess,tarfile,tomllib
ROOT=Path(__file__).resolve().parents[1]
BASE=ROOT/'.tandem-local/m5'
UPSTREAM=BASE/'upstream'

def fetch(url,path,expected):
    if path.exists():
        if hashlib.sha256(path.read_bytes()).hexdigest()!=expected:raise ValueError('Cached source differs: '+str(path))
        return
    subprocess.run(['/usr/bin/curl','--fail','--silent','--show-error','--location',url,'-o',str(path)],check=True)
    if hashlib.sha256(path.read_bytes()).hexdigest()!=expected:raise ValueError('Source fingerprint differs: '+url)

def metadata(cargo,manifest,out,locked):
    args=[str(cargo),'metadata','--format-version','1','--filter-platform','aarch64-apple-darwin','--manifest-path',str(manifest)]
    if locked:args.append('--locked')
    with out.open('w') as output:subprocess.run(args,check=True,stdout=output,cwd=ROOT)

def external(lock):
    return {(p['name'],p['version'],p.get('source'),p.get('checksum')) for p in tomllib.loads(lock)['package'] if p.get('source')}

def main():
    UPSTREAM.mkdir(parents=True,exist_ok=True)
    inputs=json.loads((ROOT/'scripts/release-inputs.json').read_text())
    fetch(inputs['codex_source_url'],UPSTREAM/'codex-source.tar.gz',inputs['codex_source_sha256'])
    fetch(inputs['mit_terms_url'],UPSTREAM/'MIT.txt',inputs['mit_terms_sha256'])
    source=UPSTREAM/'codex-rust-v0.169.0'
    with tarfile.open(UPSTREAM/'codex-source.tar.gz') as archive:
        original=archive.extractfile('codex-rust-v0.169.0/codex-rs/Cargo.lock').read().decode()
        if not source.exists():archive.extractall(UPSTREAM,filter='data')
    (UPSTREAM/'codex-Cargo.lock.original').write_text(original)
    cargo=Path.home()/'.cargo/bin/cargo'
    metadata(cargo,ROOT/'Cargo.toml',BASE/'cargo-metadata.json',True)
    # Release tagging changes internal workspace versions from 0.0.0 to
    # 0.169.0 without updating those lock entries. Reconcile the private copy,
    # then reject any change to an external dependency pin/checksum.
    metadata(cargo,source/'codex-rs/Cargo.toml',BASE/'codex-metadata.json',False)
    if external(original)!=external((source/'codex-rs/Cargo.lock').read_text()):
        raise ValueError('Codex external dependency pins changed; inspect before collecting notices')
    print('Source metadata prepared; external Codex pins match the original release lockfile.')
if __name__=='__main__':main()
