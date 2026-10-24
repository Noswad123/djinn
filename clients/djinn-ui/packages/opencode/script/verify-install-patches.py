"""Run with Python 3.12+: --bun /path/to/bun --temp-parent /approved/scratch.

Downloads public npm artifacts, verifies lockfile integrity before extraction,
and compiles standalone fixtures. Never installs into the source workspace.
"""

import argparse
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import urllib.request
import zlib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bun', required=True)
parser.add_argument('--temp-parent', required=True)
args = parser.parse_args()
if not __debug__:
    parser.error('verification requires assertions enabled; do not use -O or PYTHONOPTIMIZE')
bun = str(Path(args.bun).resolve())
ui = Path(__file__).resolve().parents[3]
scratch = Path(tempfile.mkdtemp(prefix='djinn-install-patches-', dir=args.temp_parent))
print(f'Fixtures: {scratch}', flush=True)
env = {'PATH': os.environ['PATH'], 'HOME': str(scratch / 'home'),
       'TMPDIR': str(scratch), 'BUN_INSTALL_CACHE_DIR': str(scratch / 'cache')}
for key in ['HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME']:
    env.setdefault(key, str(scratch / key.lower()))
    Path(env[key]).mkdir()

def run(*command, cwd=scratch):
    return subprocess.check_output(command, cwd=cwd, env=env, text=True)

print('Bun:', run(bun, '--version').strip(), flush=True)
lock = json.loads(run(bun, '-e',
    'console.log(JSON.stringify(Bun.JSONC.parse(await Bun.file(process.argv[1]).text())))',
    str(ui / 'bun.lock')))
manifest = json.loads((ui / 'package.json').read_text())
for name, version in [('@ff-labs/fff-bun', '0.9.4'), ('@silvia-odwyer/photon-node', '0.3.4')]:
    key = f'{name}@{version}'
    assert manifest['patchedDependencies'][key] == lock['patchedDependencies'][key]
    assert lock['packages'][name][0] == key
    for package in ['core', 'opencode']:
        assert json.loads((ui / f'packages/{package}/package.json').read_text())['dependencies'][name] == version
assert not any('fff-bun@0.9.3' in key for key in manifest['patchedDependencies'] | lock['patchedDependencies'])

platform, arch = json.loads(run(bun, '-e', 'console.log(JSON.stringify([process.platform, process.arch]))'))
# The Linux fixture targets glibc; musl selection remains in the production patch.
native = f'@ff-labs/fff-bin-{platform}-{arch}' + ('-gnu' if platform == 'linux' else '')
for name in ['@ff-labs/fff-bun', '@silvia-odwyer/photon-node', native]:
    entry = lock['packages'][name]
    version = entry[0].rsplit('@', 1)[1]
    url = f'https://registry.npmjs.org/{name}/-/{name.split("/")[-1]}-{version}.tgz'
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read()
    digest = 'sha512-' + base64.b64encode(hashlib.sha512(data).digest()).decode()
    assert digest == entry[-1], f'Integrity mismatch: {name}'
    print(f'{url}\n  {digest}', flush=True)
    unpack = scratch / 'unpack'
    unpack.mkdir()
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        archive.extractall(unpack, filter='data')
    dest = scratch / 'source/node_modules' / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(unpack / 'package', dest)
    unpack.rmdir()
    if name == native:
        continue
    patch = ui / manifest['patchedDependencies'][f'{name}@{version}']
    before = {str(p.relative_to(dest)): p.read_bytes() for p in dest.rglob('*') if p.is_file()}
    for flags in [('--check',), (), ('--reverse', '--check'), ('--reverse',)]:
        run('git', 'apply', *flags, str(patch), cwd=dest)
    assert before == {str(p.relative_to(dest)): p.read_bytes() for p in dest.rglob('*') if p.is_file()}
    run('git', 'apply', str(patch), cwd=dest)
    print(f'{name}: strict apply/reverse and pristine roundtrip PASS', flush=True)

source = scratch / 'source'
output = scratch / 'run'
output.mkdir()
# A real local 2x2 RGBA PNG, independent of Photon initialization.
def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

(output / 'input.png').write_bytes(b'\x89PNG\r\n\x1a\n' +
    chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 6, 0, 0, 0)) +
    chunk(b'IDAT', zlib.compress((b'\0' + b'\xff\0\0\xff' * 2) * 2)) + chunk(b'IEND', b''))
(source / 'photon.ts').write_text('''
import wasm from "@silvia-odwyer/photon-node/photon_rs_bg.wasm" with { type: "file" }
import assert from "node:assert/strict"
assert.ok(await Bun.file(wasm).exists())
assert.ok(wasm.includes("$bunfs"), "WASM must be embedded")
globalThis.__OPENCODE_PHOTON_WASM_PATH = wasm
const photon = await import("@silvia-odwyer/photon-node")
const image = photon.PhotonImage.new_from_byteslice(new Uint8Array(await Bun.file("input.png").arrayBuffer()))
assert.equal(image.get_width(), 2)
assert.equal(image.get_height(), 2)
const resized = photon.resize(image, 1, 1, photon.SamplingFilter.Lanczos3)
const decoded = photon.PhotonImage.new_from_byteslice(resized.get_bytes())
assert.equal(decoded.get_width(), 1)
assert.equal(decoded.get_height(), 1)
decoded.free()
resized.free()
image.free()
console.log("compiled Photon first import/decode/resize PASS")
''')
(source / 'fff.ts').write_text('''
import { FileFinder, findBinary } from "@ff-labs/fff-bun"
import assert from "node:assert/strict"
assert.ok(findBinary().includes("$bunfs"), "native library must be embedded")
assert.ok(FileFinder.isAvailable(), "native FFI required; no fallback")
const made = FileFinder.create({ basePath: process.cwd() + "/files", disableWatch: true,
  frecencyDbPath: process.cwd() + "/frecency", historyDbPath: process.cwd() + "/history" })
assert.ok(made.ok, JSON.stringify(made))
try {
  const scan = await made.value.waitForScan(10000)
  assert.ok(scan.ok && scan.value, JSON.stringify(scan))
  const result = made.value.fileSearch("needle")
  assert.ok(result.ok && result.value.items.length === 1, JSON.stringify(result))
  console.log("compiled FFF native init/scan/search PASS")
} finally {
  made.value.destroy()
}
''')
(output / 'files').mkdir()
(output / 'files/needle.txt').write_text('native search fixture\n')
for fixture in ['photon', 'fff']:
    print(run(bun, 'build', '--compile', '--minify', '--define', 'FFF_LIBC="gnu"',
        str(source / f'{fixture}.ts'), '--outfile', str(output / fixture), cwd=source), flush=True)
source.rename(scratch / 'bundled-source')
for fixture in ['photon', 'fff']:
    assert not (output / 'node_modules').exists()
    assert not list(output.glob('*.wasm'))
    print(run(str(output / fixture), cwd=output), flush=True)
