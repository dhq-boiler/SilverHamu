"""Build matched before/after production-engine test executables in a new directory."""
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import zipfile

repo = Path(__file__).resolve().parents[2]
output = Path(sys.argv[1]).resolve()
output.mkdir(parents=True, exist_ok=False)
before = output / 'before'
before.mkdir()
archive_bytes = subprocess.check_output(['git', 'archive', '--format=zip', '73d3ff692a7710f63cb14448a3b16ca5f80d7f30'], cwd=repo)
with zipfile.ZipFile(io.BytesIO(archive_bytes)) as archive:
    for name in archive.namelist():
        if not (before / name).resolve().is_relative_to(before):
            raise ValueError('Archive path escapes output')
    archive.extractall(before)
bench = before / 'benchmarks/global-generation'
bench.mkdir()
shutil.copy2(repo / 'benchmarks/global-generation/engine_bench.rs', bench / 'engine_bench.rs')
source = before / 'src/transactions.rs'
source.write_text(source.read_text(encoding='utf-8') + '\n#[cfg(test)]\n#[path = "../benchmarks/global-generation/engine_bench.rs"]\nmod generation_guard_bench;\n', encoding='utf-8')
binaries = {}
for mode, directory in [('before', before), ('after', repo)]:
    run = subprocess.run([os.environ.get('CARGO', 'cargo'), 'test', '--locked', '--release', '--features', 'service', '--lib', '--no-run', '--message-format=json'], cwd=directory, capture_output=True, text=True)
    print(run.stderr)
    if run.returncode:
        print(run.stdout)
        raise SystemExit(run.returncode)
    for line in run.stdout.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'compiler-artifact' and item.get('executable') and item['profile']['test']:
            binaries[mode] = item['executable']
assert len(binaries) == 2
(output / 'binaries.json').write_text(json.dumps(binaries, indent=2), encoding='utf-8')
