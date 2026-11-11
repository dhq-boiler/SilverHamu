"""Exercise all six SDKs against fresh token and TLS silverhamu services."""
import argparse
from contextlib import contextmanager
import importlib.util
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
SDK = ROOT / "clients"
spec = importlib.util.spec_from_file_location("silverhamu_reference", ROOT / "scripts/silverhamu-client.py")
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)


def commands():
    java = SDK / "java/target"
    classpath = os.pathsep.join(str(java / path) for path in ("classes", "test-classes", "dependency/*"))
    return {
        "python": [sys.executable, SDK / "python/smoke.py"],
        "node": ["node", SDK / "node/smoke.mjs"],
        "go": [os.getenv("CATDB_GO_SMOKE", ROOT / "artifacts" / ("go-smoke.exe" if os.name == "nt" else "go-smoke"))],
        "dotnet": ["dotnet", os.getenv("CATDB_DOTNET_SMOKE", SDK / "dotnet/smoke/bin/Release/net10.0/Smoke.dll")],
        "java": ["java", "-cp", classpath, "io.github.dhqboiler.silverhamu.Smoke"],
        "ruby": ["ruby", SDK / "ruby/smoke.rb"],
    }


def certificates(folder):
    conf = folder / "openssl.cnf"
    conf.write_text("[req]\ndistinguished_name=dn\n[dn]\n", encoding="ascii")
    leaf = folder / "leaf.ext"
    leaf.write_text("subjectAltName=DNS:localhost\nbasicConstraints=critical,CA:FALSE\n"
                    "keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n", encoding="ascii")

    def openssl(*args):
        subprocess.run([os.getenv("OPENSSL", "openssl"), *args], cwd=folder, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)

    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-config", str(conf),
            "-subj", "/CN=silverhamu SDK test CA", "-addext", "basicConstraints=critical,CA:TRUE",
            "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-keyout", "ca.key", "-out", "ca.pem")
    openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-config", str(conf), "-subj", "/CN=localhost",
            "-keyout", "server.key", "-out", "server.csr")
    openssl("x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
            "-CAcreateserial", "-days", "1", "-extfile", str(leaf), "-out", "server.pem")


@contextmanager
def server(binary, folder, flags):
    proc = subprocess.Popen([binary, "--bind", "127.0.0.1:0", "--db", folder / "data.ndb", "--init", *flags],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        ready = proc.stdout.readline()
        match = re.search(r"ready 127\.0\.0\.1:(\d+) ", ready)
        if not match:
            proc.terminate()
            _, errors = proc.communicate(timeout=10)
            raise RuntimeError("silverhamu failed to start: " + ready + errors)
        yield int(match.group(1))
    finally:
        proc.terminate()
        try:
            proc.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate()


def smoke(command, env, label, *, reject=False):
    result = subprocess.run(command, env=env, timeout=30, capture_output=True, text=True, errors="replace")
    if (result.returncode != 0) != reject:
        detail = (result.stdout + result.stderr)[-1800:]
        for key in ("CATDB_TOKEN", "CATDB_PASSWORD"):
            if env.get(key):
                detail = detail.replace(env[key], "[redacted]")
        raise AssertionError(f"{label} returned {result.returncode}\n{detail}")


def exercise(binary, folder, tls, clients):
    secret = secrets.token_hex(32)
    credential = folder / "secret"
    credential.write_text(secret, encoding="ascii")
    env = {k: v for k, v in os.environ.items() if k not in {
        "CATDB_HOST", "CATDB_PORT", "CATDB_TOKEN", "CATDB_USER", "CATDB_PASSWORD",
        "CATDB_CA_FILE", "CATDB_AUTH_ONLY", "CATDB_EXPECT", "CATDB_VALUE"}}
    if tls:
        certificates(folder)
        flags = ["--tls-cert", folder / "server.pem", "--tls-key", folder / "server.key",
                 "--bootstrap-admin", "owner", "--bootstrap-password-file", credential]
        auth = dict(username="owner", password=secret, cafile=str(folder / "ca.pem"))
        env.update(CATDB_HOST="localhost", CATDB_USER="owner", CATDB_PASSWORD=secret, CATDB_CA_FILE=auth["cafile"])
    else:
        flags = ["--token-file", credential]
        auth = dict(token=secret)
        env.update(CATDB_HOST="127.0.0.1", CATDB_TOKEN=secret)
    with server(binary, folder, flags) as port, reference.Client(env["CATDB_HOST"], port, **auth) as observer:
        env["CATDB_PORT"] = str(port)
        prior = "cafe"
        assert observer.request(op="put_commit", key="ff00", value=prior)["ok"]
        mode = "TLS" if tls else "token"
        for index, (language, command) in enumerate(clients.items(), 1):
            value = f"00ff{index:02x}80"
            smoke(command, dict(env, CATDB_EXPECT=prior, CATDB_VALUE=value), f"{mode}/{language}")
            assert observer.request(op="get_latest", key="ff00")["value"] == value
            print(f"{mode}/{language} binary round trip and transactions passed", flush=True)
            prior = value
        for language, command in clients.items():
            bad = dict(env, CATDB_AUTH_ONLY="1")
            bad["CATDB_PASSWORD" if tls else "CATDB_TOKEN"] = "incorrect-" + secrets.token_hex(32)
            smoke(command, bad, f"{mode}/{language}/bad credentials", reject=True)
            if tls:
                untrusted = dict(env, CATDB_AUTH_ONLY="1")
                del untrusted["CATDB_CA_FILE"]
                smoke(command, untrusted, f"{language}/untrusted CA", reject=True)
                smoke(command, dict(env, CATDB_AUTH_ONLY="1", CATDB_HOST="127.0.0.1"),
                      f"{language}/wrong hostname", reject=True)
        print(f"{mode} authentication rejection checks passed for {len(clients)} SDKs", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/debug" /
                        ("silverhamu-server.exe" if os.name == "nt" else "silverhamu-server"))
    parser.add_argument("--clients", nargs="+", choices=list(commands()), default=list(commands()))
    args = parser.parse_args()
    clients = {name: commands()[name] for name in args.clients}
    for tls in (False, True):
        with tempfile.TemporaryDirectory() as tmp:
            exercise(args.server.resolve(strict=True), Path(tmp), tls, clients)
