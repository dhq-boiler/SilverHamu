import pathlib
import subprocess
import sys

binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
output = pathlib.Path(sys.argv[2]).resolve()
output.mkdir(parents=True, exist_ok=False)
for repeat in range(1, 4):
    for size in [100000, 1000000]:
        for shape in ['local', 'spread']:
            order = 'max-first' if repeat % 2 == 0 else 'plain-first'
            subprocess.run([str(binary), str(size), shape, order, str(output / f'{size}-{shape}-{repeat}.csv')], check=True)
