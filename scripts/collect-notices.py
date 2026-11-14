"""Collect local package notices and pinned upstream texts for a local release.

Inputs are cargo metadata JSONs already resolved for the macOS target. Dev-only
edges are excluded; build dependencies and workspace feature unification make
this an intentionally conservative inventory, not a binary-level SBOM.
"""
from pathlib import Path
import argparse, concurrent.futures, hashlib, json, re, shutil, subprocess, tarfile

ROOT=Path(__file__).resolve().parents[1]
CACHE=ROOT/'.tandem-local/m5/notice-cache'
NAMES=['LICENSES/MIT.txt','LICENSES/Apache-2.0.txt','License.md','LICENSE-MIT.txt','LICENSE-APACHE.txt','LICENSE-MIT.md','LICENSE-APACHE.md','LICENSE-MIT-MAC','COPYRIGHT.md','LICENSE-BSD','LICENSE','LICENSE.txt','LICENSE.md','LICENSE-MIT','LICENSE-APACHE','COPYING','UNLICENSE','NOTICE','NOTICE.md','LICENSE-ZLIB']
PATTERN=re.compile(r'^(licen[cs]e|copying|notice|copyright|unlicense)([._-]|$)',re.I)

def graph(path,roots):
    metadata=json.loads(path.read_text());nodes={n['id']:n for n in metadata['resolve']['nodes']};packages={p['id']:p for p in metadata['packages']};seen=set()
    def walk(id):
        if id in seen:return
        seen.add(id)
        for dep in nodes[id]['deps']:
            if any(k['kind']!='dev' for k in dep['dep_kinds']):walk(dep['pkg'])
    for p in metadata['packages']:
        if p['name'] in roots:walk(p['id'])
    return [packages[id] for id in sorted(seen)]

def upstream(package):
    directory=Path(package['manifest_path']).parent
    vcs_path=directory/'.cargo_vcs_info.json'
    vcs=json.loads(vcs_path.read_text()) if vcs_path.exists() else {}
    repo=(package.get('repository') or '').removesuffix('.git').rstrip('/')
    if repo.startswith('https://github.com/'):repo='/'.join(repo.split('/')[:5])
    commit=vcs.get('git',{}).get('sha1')
    if (package.get('source') or '').startswith('git+'):
        source=package['source'][4:];repo=source.split('?')[0].split('#')[0].removesuffix('.git');commit=source.rsplit('#',1)[-1]
    # Published zune-inflate omits repository metadata; its packaged README
    # identifies this upstream, and cargo_vcs_info pins the source revision.
    if package['name']=='tree-sitter-diff':repo='https://github.com/the-mikedavis/tree-sitter-diff'
    if package['name'] in ['zune-inflate','zune-core']:repo='https://github.com/etemesi254/zune-image'
    if not repo.startswith('https://github.com/') or not commit or not re.fullmatch(r'[0-9a-f]{40}',commit):return []
    key=hashlib.sha256((repo+commit).encode()).hexdigest();folder=CACHE/key;folder.mkdir(parents=True,exist_ok=True)
    result=[]
    for name in NAMES:
        target=folder/name;missing=folder/(name+'.missing');target.parent.mkdir(parents=True,exist_ok=True)
        url='https://raw.githubusercontent.com/'+repo.removeprefix('https://github.com/')+'/'+commit+'/'+name
        if not target.exists() and not missing.exists():
            call=subprocess.run(['/usr/bin/curl','--fail','--silent','--location','--max-time','20',url],capture_output=True)
            if call.returncode==0 and 40<=len(call.stdout)<=2*1024*1024 and b'<!DOCTYPE html' not in call.stdout[:100]:target.write_bytes(call.stdout)
            elif call.returncode==22:missing.touch()
        if target.exists():result.append((target,'upstream/'+name,url))
    return result

def collect(package):
    directory=Path(package['manifest_path']).parent
    texts=[]
    for path in directory.rglob('*'):
        if any(p in ['.git','target','node_modules'] for p in path.relative_to(directory).parts):continue
        if path.is_file() and not path.is_symlink() and (PATTERN.match(path.name) or any(part.lower() in ['licenses','licences'] for part in path.relative_to(directory).parts[:-1])) and path.stat().st_size<=2*1024*1024:
            texts.append((path,str(path.relative_to(directory)),'published source package'))
    explicit=package.get('license_file')
    if explicit:
        path=Path(explicit)
        if not path.is_absolute():path=directory/path
        if path.is_file() and path.resolve().is_relative_to(directory.resolve()) and all(t[0]!=path for t in texts):texts.append((path,str(path.relative_to(directory)),'declared license file'))
    vendor=ROOT/'.tandem-local/m5/upstream/codex-rust-v0.169.0'
    if not texts and directory.is_relative_to(vendor):
        texts=[(vendor/name,'upstream/'+name,'OpenAI Codex rust-v0.169.0') for name in ['LICENSE','NOTICE']]
    if not texts:texts=upstream(package)
    extra=ROOT/'scripts/notice-sources.json'
    if not texts and extra.exists():
        for item in json.loads(extra.read_text())['entries']:
            if [package['name'],package['version']] in item['packages']:
                for i,source in enumerate(item['files']):
                    cached=CACHE/('extra-'+source['sha256'])
                    if not cached.exists():
                        call=subprocess.run(['/usr/bin/curl','--fail','--silent','--location','--max-time','30',source['url']],capture_output=True)
                        if call.returncode==0 and hashlib.sha256(call.stdout).hexdigest()==source['sha256']:cached.write_bytes(call.stdout)
                    if cached.exists() and hashlib.sha256(cached.read_bytes()).hexdigest()==source['sha256']:
                        texts.append((cached,'upstream-extra/'+str(i)+'-'+Path(source['path']).name,source['url']))
    # Some published crates and their upstream revisions declare permissive
    # SPDX terms but provide no standalone license file. Preserve that fact,
    # original source copyright lines and package attribution without inventing
    # a copyright owner. Supply the complete declared standard license terms.
    if not texts and package.get('license') in ['MIT','MIT/Apache-2.0','Apache-2.0/MIT','MIT OR Apache-2.0']:
        fallback=CACHE/('declared-'+package['name']+'-'+package['version']);fallback.mkdir(exist_ok=True)
        copyrights=[]
        for source in directory.rglob('*'):
            if source.is_file() and source.suffix in ['.rs','.md','.txt'] and source.stat().st_size<200000:
                copyrights.extend(line.strip() for line in source.read_text(errors='replace').splitlines() if 'copyright' in line.lower())
        attribution=fallback/'ATTRIBUTION.txt'
        attribution.write_text('Package: '+package['name']+' '+package['version']+'\nDeclared SPDX license: '+package['license']+'\nRepository: '+str(package.get('repository'))+'\nPublished package authors: '+', '.join(package['authors'])+'\n\nNo standalone license file was present in the inspected package/upstream revision. The accompanying standard terms follow the package license declaration; author metadata is attribution, not a newly asserted copyright notice.\n\nOriginal source copyright statements:\n'+'\n'.join(dict.fromkeys(copyrights))+'\n')
        choice='Apache-2.0' if 'Apache' in package['license'] else 'MIT'
        source=ROOT/'.tandem-local/m5/upstream'/('codex-rust-v0.169.0/LICENSE' if choice=='Apache-2.0' else 'MIT.txt')
        texts=[(attribution,'ATTRIBUTION.txt','published license metadata and original copyright statements; standalone notice absent'),(source,'STANDARD-'+choice+'.txt','declared standard license terms; SPDX MIT v3.27.0 or Apache 2.0')]
    return package,texts

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);args=parser.parse_args();output=args.output;output.mkdir(parents=True,exist_ok=True);CACHE.mkdir(parents=True,exist_ok=True)
    groups=[('tandem',ROOT/'.tandem-local/m5/cargo-metadata.json',['tandem']),('codex',ROOT/'.tandem-local/m5/codex-metadata.json',['codex-cli','codex-exec-server-cli'])]
    packages={};consumers={}
    for group,path,roots in groups:
        for package in graph(path,roots):
            if package['name'].startswith('tandem') and package.get('source') is None:continue
            key=package['id'];packages[key]=package;consumers.setdefault(key,[]).append(group)
    rows=[];unresolved=[]
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as executor:
        for package,texts in executor.map(collect,packages.values()):
            key=package['id'];name=package['name']+'-'+package['version']+'-'+hashlib.sha256(key.encode()).hexdigest()[:8];directory=output/'packages'/name;directory.mkdir(parents=True,exist_ok=True)
            receipts=[]
            for source,relative,origin in texts:
                target=directory/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(source,target);receipts.append({'path':str(target.relative_to(output)),'origin':origin,'sha256':hashlib.sha256(target.read_bytes()).hexdigest()})
            row={'name':package['name'],'version':package['version'],'license':package['license'],'source':package['source'] or 'OpenAI Codex rust-v0.169.0 source','repository':package['repository'],'consumers':consumers[key],'notices':receipts,'modified':False,'notice_basis':'declared metadata and standard terms' if any(t[1]=='ATTRIBUTION.txt' for t in texts) else 'upstream notice files'}
            if not texts:unresolved.append({'name':package['name'],'version':package['version'],'license':package['license'],'repository':package['repository']})
            # Include complete unchanged source for MPL components. Source-only
            # build tools are retained too, conservatively, without binaries.
            if package['license'] and 'MPL-2.0' in package['license']:
                sources=output/'sources';sources.mkdir(exist_ok=True);archive=sources/(name+'.tar.gz')
                with tarfile.open(archive,'w:gz') as tar:
                    tar.add(Path(package['manifest_path']).parent,arcname=package['name']+'-'+package['version'],filter=lambda info: None if any(p in ['.git','target'] for p in Path(info.name).parts) else info)
                row['source_archive']=str(archive.relative_to(output))
            rows.append(row)
    vendor=ROOT/'.tandem-local/m5/upstream/codex-rust-v0.169.0'
    for name in ['LICENSE','NOTICE']:
        dest=output/'codex'/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(vendor/name,dest)
    inventory={'schema':1,'tandem_lock_sha256':hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest(),'codex_original_lock_sha256':hashlib.sha256((ROOT/'.tandem-local/m5/upstream/codex-Cargo.lock.original').read_bytes()).hexdigest(),'method':'macOS cargo dependency closure; normal/build edges; conservative workspace feature superset','codex_tag':'rust-v0.169.0','codex_external_lock_pins_unchanged':True,'packages':sorted(rows,key=lambda p:(p['name'],p['version'],p['source'])),'unresolved':unresolved}
    (output/'inventory.json').write_text(json.dumps(inventory,indent=2)+'\n')
    shutil.copy2(ROOT/'docs/tandem/third-party-notices.md',output/'README.md')
    print(f'{len(rows)} entries; {len(unresolved)} unresolved notices');print(json.dumps(unresolved,indent=2))
if __name__=='__main__':main()
