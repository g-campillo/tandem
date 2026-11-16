"""Prepare reproducible source metadata for the local notice collector; no build/install."""
from pathlib import Path
import hashlib,json,subprocess
from macos_release import check_helper_pins
ROOT=Path(__file__).resolve().parents[1]
BASE=ROOT/'.tandem-local/m5'
UPSTREAM=BASE/'upstream'

def fetch(url,path,expected):
    if path.exists():
        if hashlib.sha256(path.read_bytes()).hexdigest()!=expected:raise ValueError('Cached source differs: '+str(path))
        return
    subprocess.run(['/usr/bin/curl','--fail','--silent','--show-error','--location',url,'-o',str(path)],check=True)
    if hashlib.sha256(path.read_bytes()).hexdigest()!=expected:raise ValueError('Source fingerprint differs: '+url)

def metadata(cargo,manifest,out):
    args=[str(cargo),'metadata','--format-version','1','--filter-platform','aarch64-apple-darwin','--manifest-path',str(manifest),'--locked']
    with out.open('w') as output:subprocess.run(args,check=True,stdout=output,cwd=ROOT)

def main():
    UPSTREAM.mkdir(parents=True,exist_ok=True)
    inputs=json.loads((ROOT/'scripts/release-inputs.json').read_text())
    fetch(inputs['mit_terms_url'],UPSTREAM/'MIT.txt',inputs['mit_terms_sha256'])
    fetch(inputs['apache_terms_url'],UPSTREAM/'Apache-2.0.txt',inputs['apache_terms_sha256'])
    metadata(Path.home()/'.cargo/bin/cargo',ROOT/'Cargo.toml',BASE/'cargo-metadata.json')
    # The npm half of the inventory is read from the installed helper, so it
    # must be the pinned install that packaging will bundle.
    check_helper_pins()
    print('Source metadata prepared; the Claude helper matches its pins.')
if __name__=='__main__':main()
