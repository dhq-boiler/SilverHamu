"""Run the bounded single-writer comparison using an already built test image."""
import argparse
import json
from pathlib import Path
import subprocess
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--image', default='catdb-reuse-speed')
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
volume = 'catdb-reuse-speed-' + uuid.uuid4().hex
subprocess.run(['docker', 'volume', 'create', volume], check=True)
(args.output / 'volume.txt').write_text(volume + '\n')
image = subprocess.check_output(['docker', 'image', 'inspect', args.image, '--format', '{{.Id}}'], text=True).strip()
order = [('clear', 1), ('reuse', 1), ('reuse', 2), ('clear', 2), ('clear', 3), ('reuse', 3)]
(args.output / 'environment.json').write_text(json.dumps({'image': image, 'volume': volume, 'cpu_quota': 2, 'memory': '4g', 'order': order}, indent=2) + '\n')
for mode, trial in order:
    command = ['docker', 'run', '--rm', '--cpus=2', '--memory=4g', '--mount', f'type=volume,source={volume},target=/data', '-e', f'REUSE_SPEED_MODE={mode}', '-e', f'REUSE_SPEED_TRIAL={trial}', '-e', 'REUSE_SPEED_ROOT=/data', args.image, 'cargo', 'test', '--locked', '--release', '--lib', 'transactions::variable_gate::tests::reuse_speed', '--', '--exact', '--ignored', '--nocapture', '--test-threads=1']
    with (args.output / f'{mode}-{trial}.log').open('w', encoding='utf-8') as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, text=True, check=True)
    print(f'Completed {mode} trial {trial}', flush=True)
print(f'Volume retained for inspection: {volume}')
