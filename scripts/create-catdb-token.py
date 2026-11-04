"""Create a random catdb token without overwriting an existing credential."""
import argparse
import os
import pathlib
import secrets

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("path", nargs="?", type=pathlib.Path, default=pathlib.Path(".secrets/catdb-token"))
args = parser.parse_args()
args.path.parent.mkdir(parents=True, exist_ok=True)
descriptor = os.open(args.path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(descriptor, "w", encoding="utf-8") as output:
    output.write(secrets.token_hex(32))
    output.flush()
    os.fsync(output.fileno())
print(f"Created {args.path}; token is not printed.")
