"""Artifact work runs as a supervised job, with progress and an atomic destination."""
import fnmatch
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

LIMIT = 1024 * 1024 * 1024


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def request(url):
    if not url.startswith(('https://', 'http://')):
        raise ValueError('URL must use HTTPS or HTTP')
    return urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': 'Toad-Computer'}), timeout=30)


def source_url(spec):
    if spec.get('url'):
        return spec['url']
    repo = spec.get('repo', '')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
        raise ValueError('url or GitHub owner/repo is required')
    version = spec.get('version') or 'latest'
    if not re.fullmatch(r'[A-Za-z0-9_.-]+', version):
        raise ValueError('invalid release version')
    endpoint = 'latest' if version == 'latest' else 'tags/' + version
    with request(f'https://api.github.com/repos/{repo}/releases/{endpoint}') as response:
        release = json.load(response)
    architecture = {'aarch64': 'arm64', 'x86_64': 'amd64'}.get(platform.machine(), platform.machine())
    pattern = (spec.get('asset') or '').replace('{arch}', architecture).replace('{os}', 'linux')
    if not pattern:
        raise ValueError('asset filename/pattern is required; {arch} and {os} are supported')
    matches = [asset for asset in release['assets'] if fnmatch.fnmatchcase(asset['name'], pattern)]
    if len(matches) != 1:
        raise ValueError(f'asset pattern matched {len(matches)} files; choose one exact asset')
    print(json.dumps({'release': release['tag_name'], 'asset': matches[0]['name'], 'architecture': architecture}), flush=True)
    return matches[0]['browser_download_url']


def download(spec, destination):
    expected = (spec.get('sha256') or '').lower()
    if expected and not re.fullmatch('[0-9a-f]{64}', expected):
        raise ValueError('sha256 must contain 64 hexadecimal digits')
    if destination.exists():
        if expected and digest(destination) == expected:
            print(json.dumps({'path': str(destination), 'sha256': expected, 'verified': True, 'cached': True}), flush=True)
            return
        raise ValueError('destination exists; use a matching sha256 for a cache hit or choose another path')
    url = source_url(spec)
    temporary = destination.with_name(destination.name + f'.{os.getpid()}.part')
    try:
        checksum = hashlib.sha256()
        total = 0
        with request(url) as response, temporary.open('xb') as output:
            length = int(response.headers.get('Content-Length', '0'))
            if length > LIMIT:
                raise ValueError('artifact exceeds 1 GiB')
            while block := response.read(1024 * 1024):
                total += len(block)
                if total > LIMIT:
                    raise ValueError('artifact exceeds 1 GiB')
                output.write(block)
                checksum.update(block)
                print(f'Downloaded {total} bytes' + (f' of {length}' if length else ''), flush=True)
            if length and length != total:
                raise ValueError('download was incomplete')
        actual = checksum.hexdigest()
        if expected and actual != expected:
            raise ValueError(f'SHA-256 mismatch: expected {expected}, got {actual}')
        # Linking refuses to overwrite a destination created during the download.
        os.link(temporary, destination)
        print(json.dumps({'path': str(destination), 'bytes': total, 'sha256': actual, 'verified': bool(expected), 'cached': False}), flush=True)
    finally:
        temporary.unlink(missing_ok=True)


def member_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or '\\' in name:
        raise ValueError('archive entry escapes the destination')
    return path


def extract(archive, destination):
    if destination.exists():
        raise ValueError('extraction destination already exists; choose an empty new directory')
    temporary = Path(tempfile.mkdtemp(prefix='.toad-extract-', dir=destination.parent))
    total = 0
    try:
        if zipfile.is_zipfile(archive):
            with zipfile.ZipFile(archive) as bundle:
                for entry in bundle.infolist():
                    member_path(entry.filename)
                    if stat.S_ISLNK(entry.external_attr >> 16):
                        raise ValueError('archive symlinks are not supported')
                    total += entry.file_size
                    if total > LIMIT:
                        raise ValueError('expanded archive exceeds 1 GiB')
                bundle.extractall(temporary)
                for entry in bundle.infolist():
                    path = temporary / entry.filename
                    if path.is_file() and entry.external_attr >> 16 & 0o111:
                        path.chmod(0o755)
        else:
            with tarfile.open(archive) as bundle:
                members = bundle.getmembers()
                for entry in members:
                    member_path(entry.name)
                    if not (entry.isfile() or entry.isdir()):
                        raise ValueError('archive links and special files are not supported')
                    total += entry.size
                    if total > LIMIT:
                        raise ValueError('expanded archive exceeds 1 GiB')
                bundle.extractall(temporary, members=members, filter='data')
        os.rename(temporary, destination)
        print(json.dumps({'path': str(destination), 'expanded_bytes': total}), flush=True)
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)


def main(spec):
    path = Path(spec['path'])
    action = spec['action']
    if action == 'extract':
        extract(path, Path(spec['destination']))
    else:
        if action == 'download' or spec.get('url') or spec.get('repo'):
            download(spec, path)
        if action == 'run':
            expected = spec.get('sha256')
            if expected and digest(path) != expected.lower():
                raise ValueError('script checksum does not match; refusing to run')
            interpreter = spec.get('interpreter') or 'bash'
            if interpreter not in ('bash', 'sh', 'python3'):
                raise ValueError('interpreter must be bash, sh, or python3')
            print(f'Running {path} with {interpreter}', flush=True)
            os.execvp(interpreter, [interpreter, str(path), *spec.get('args', [])])


if __name__ == '__main__':
    try:
        main(json.loads(sys.argv[1]))
    except Exception as error:
        print(f'Artifact failed: {error}', file=sys.stderr, flush=True)
        sys.exit(1)
