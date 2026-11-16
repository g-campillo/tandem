"""Collect local package notices and pinned upstream texts for a local release.

Inputs are Tandem's cargo metadata, already resolved for the macOS target, and
the installed npm tree of the Claude helper. Dev-only cargo edges are excluded;
build dependencies and workspace feature unification make this an intentionally
conservative inventory, not a binary-level SBOM.
"""
from pathlib import Path
import argparse, concurrent.futures, hashlib, json, re, shutil, subprocess, tarfile

ROOT=Path(__file__).resolve().parents[1]
CACHE=ROOT/'.tandem-local/m5/notice-cache'
UPSTREAM=ROOT/'.tandem-local/m5/upstream'
HELPER=ROOT/'crates/tandem-claude/helper'
FALLBACK_SENTENCE='No standalone license file was present in the published package; the accompanying standard terms follow its license declaration.'
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
        source=UPSTREAM/(choice+'.txt')
        texts=[(attribution,'ATTRIBUTION.txt','published license metadata and original copyright statements; standalone notice absent'),(source,'STANDARD-'+choice+'.txt','declared standard license terms; SPDX v3.27.0')]
    return package,texts

def repository_url(manifest):
    repository=manifest.get('repository')
    return repository.get('url') if isinstance(repository,dict) else repository

def collect_npm(helper,output):
    """Notice rows for every installed package in the helper's lockfile, and the unresolved ones."""
    lock=json.loads((helper/'package-lock.json').read_text());rows=[];unresolved=[];seen=set()
    for key,entry in sorted(lock['packages'].items()):
        directory=helper/key
        # Optional platform packages for other systems are locked but not installed.
        if not key.startswith('node_modules/') or not directory.is_dir():continue
        manifest=json.loads((directory/'package.json').read_text())
        name=key.rsplit('node_modules/',1)[1];version=entry.get('version') or manifest['version']
        if (name,version) in seen:continue
        seen.add((name,version))
        license=entry.get('license') or manifest.get('license');repository=repository_url(manifest)
        target=output/'npm'/(name.replace('/','__')+'-'+version);texts=[]
        for path in sorted(directory.iterdir()):
            if path.is_file() and not path.is_symlink() and PATTERN.match(path.name) and path.stat().st_size<=2*1024*1024:
                texts.append((path,path.name,'published npm package'))
        if not texts and license in ['MIT','Apache-2.0']:
            target.mkdir(parents=True,exist_ok=True);attribution=target/'ATTRIBUTION.txt'
            attribution.write_text('Package: '+name+' '+version+'\nDeclared license: '+license+'\nRepository: '+str(repository)+'\n\n'+FALLBACK_SENTENCE+'\n')
            texts=[(attribution,'ATTRIBUTION.txt','published license metadata; standalone notice absent'),(UPSTREAM/(license+'.txt'),'STANDARD-'+license+'.txt','declared standard license terms; SPDX v3.27.0')]
        receipts=[]
        for source,relative,origin in texts:
            destination=target/relative;destination.parent.mkdir(parents=True,exist_ok=True)
            if source!=destination:shutil.copy2(source,destination)
            receipts.append({'path':str(destination.relative_to(output)),'origin':origin,'sha256':hashlib.sha256(destination.read_bytes()).hexdigest()})
        rows.append({'ecosystem':'npm','name':name,'version':version,'license':license,'source':entry.get('resolved'),'repository':repository,'consumers':['claude-helper'],'notices':receipts,'modified':False,'notice_basis':'declared metadata and standard terms' if any(t[1]=='ATTRIBUTION.txt' for t in texts) else 'upstream notice files'})
        if not texts:unresolved.append({'ecosystem':'npm','name':name,'version':version,'license':license,'repository':repository})
    return rows,unresolved

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);args=parser.parse_args();output=args.output;output.mkdir(parents=True,exist_ok=True);CACHE.mkdir(parents=True,exist_ok=True)
    groups=[('tandem',ROOT/'.tandem-local/m5/cargo-metadata.json',['tandem'])]
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
            row={'ecosystem':'cargo','name':package['name'],'version':package['version'],'license':package['license'],'source':package['source'] or 'local','repository':package['repository'],'consumers':consumers[key],'notices':receipts,'modified':False,'notice_basis':'declared metadata and standard terms' if any(t[1]=='ATTRIBUTION.txt' for t in texts) else 'upstream notice files'}
            if not texts:unresolved.append({'ecosystem':'cargo','name':package['name'],'version':package['version'],'license':package['license'],'repository':package['repository']})
            # Include complete unchanged source for MPL components. Source-only
            # build tools are retained too, conservatively, without binaries.
            if package['license'] and 'MPL-2.0' in package['license']:
                sources=output/'sources';sources.mkdir(exist_ok=True);archive=sources/(name+'.tar.gz')
                with tarfile.open(archive,'w:gz') as tar:
                    tar.add(Path(package['manifest_path']).parent,arcname=package['name']+'-'+package['version'],filter=lambda info: None if any(p in ['.git','target'] for p in Path(info.name).parts) else info)
                row['source_archive']=str(archive.relative_to(output))
            rows.append(row)
    npm_rows,npm_unresolved=collect_npm(HELPER,output);rows+=npm_rows;unresolved+=npm_unresolved
    sdk=json.loads((HELPER/'package-lock.json').read_text())['packages']['node_modules/@anthropic-ai/claude-agent-sdk']['version']
    inventory={'schema':1,'tandem_lock_sha256':hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest(),'helper_lock_sha256':hashlib.sha256((HELPER/'package-lock.json').read_bytes()).hexdigest(),'claude_agent_sdk_version':sdk,'method':'macOS cargo dependency closure (normal and build edges, workspace feature superset) plus the installed npm tree of the Claude helper','packages':sorted(rows,key=lambda p:(p['ecosystem'],p['name'],p['version'],p['source'] or '')),'unresolved':unresolved}
    (output/'inventory.json').write_text(json.dumps(inventory,indent=2)+'\n')
    shutil.copy2(ROOT/'docs/tandem/third-party-notices.md',output/'README.md')
    print(f'{len(rows)} entries; {len(unresolved)} unresolved notices');print(json.dumps(unresolved,indent=2))
if __name__=='__main__':main()
