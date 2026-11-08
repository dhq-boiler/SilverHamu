"""Isolated Linux container checks; never mounts an existing database."""
import argparse
import importlib.util
import json
import pathlib
import secrets
import subprocess
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("client", ROOT / "scripts/catdb-client.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def docker(*args, check=True):
    result = subprocess.run(["docker", *args], capture_output=True, text=True, timeout=60)
    if check and result.returncode:
        raise RuntimeError(result.stderr)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default="catdb:local")
    image = parser.parse_args().image
    name = "catdb-check-" + uuid.uuid4().hex[:12]
    volume = name + "-data"
    (ROOT / "artifacts").mkdir(exist_ok=True)
    folder = pathlib.Path(tempfile.mkdtemp(prefix="service-container-", dir=ROOT / "artifacts"))
    token = secrets.token_hex(32)
    secret = folder / "token"
    secret.write_text(token)
    passed = []
    docker("volume", "create", volume)

    def start():
        docker("run", "--detach", "--name", name, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--stop-timeout", "10", "-p", "127.0.0.1::7400", "--mount", f"type=volume,source={volume},target=/data", "--mount", f"type=bind,source={secret},target=/run/secrets/catdb_token,readonly", image)
        port = int(docker("port", name, "7400/tcp").stdout.strip().rsplit(":", 1)[1])
        deadline = time.monotonic() + 20
        while True:
            try:
                with module.Client("127.0.0.1", port, token) as client:
                    assert client.request(op="ping")["ok"]
                return port
            except (OSError, RuntimeError):
                if time.monotonic() > deadline:
                    raise RuntimeError(docker("logs", name).stderr)
                time.sleep(.1)

    def ok(client, **request):
        result = client.request(**request)
        assert result["ok"], result
        return result

    try:
        port = start()
        assert docker("exec", name, "id", "-u").stdout.strip() == "10001"
        fmt = docker("exec", name, "od", "-An", "-t", "u4", "-j8", "-N4", "/data/state.ndb")
        chunks = docker("exec", name, "od", "-An", "-t", "u4", "-j40", "-N4", "/data/state.ndb")
        assert int(fmt.stdout) == 4 and int(chunks.stdout) & (1 << 31)
        passed.append("New database uses managed WAL format 4 without environment settings")
        docker("exec", name, "catdb-server", "--healthcheck", "--token-file", "/run/secrets/catdb_token")
        state = json.loads(docker("inspect", name).stdout)[0]
        assert state["HostConfig"]["ReadonlyRootfs"]
        passed.append("Docker non-root, readonly root filesystem, authenticated healthcheck")
        with module.Client("127.0.0.1", port, token) as client:
            ok(client, op="begin");ok(client, op="put", key="01", value="aabb");ok(client, op="commit")
            docker("exec", name, "chmod", "555", "/data")
            assert client.request(op="checkpoint")["error"] == "storage_error"
            docker("exec", name, "chmod", "755", "/data")
            ok(client, op="begin")
            assert ok(client, op="get", key="01")["value"] == "aabb"
            ok(client, op="rollback");ok(client, op="checkpoint")
        passed.append("ER checkpoint storage failure preserves committed data; retry succeeds")
        client = module.Client("127.0.0.1", port, token)
        ok(client, op="begin");ok(client, op="put", key="02", value="ff")
        docker("stop", name)
        assert json.loads(docker("inspect", name).stdout)[0]["State"]["ExitCode"] == 0
        client.close();docker("rm", name)
        port = start()
        with module.Client("127.0.0.1", port, token) as client:
            ok(client, op="begin")
            assert ok(client, op="get", key="01")["value"] == "aabb"
            assert ok(client, op="get", key="02")["value"] is None
            ok(client, op="put", key="03", value="cc");ok(client, op="commit")
        passed.append("ST SIGTERM with open transaction exits 0; recreated container restores volume")
        client = module.Client("127.0.0.1", port, token)
        ok(client, op="begin");ok(client, op="put", key="04", value="dd")
        docker("kill", name);client.close();docker("rm", name)
        port = start()
        with module.Client("127.0.0.1", port, token) as client:
            ok(client, op="begin")
            assert ok(client, op="scan", start="")["rows"] == [["01", "aabb"], ["03", "cc"]]
        passed.append("ER SIGKILL and recreation preserve every acknowledged value, discard staged value")
        (folder / "result.json").write_text(json.dumps({"passed":passed}, indent=2))
        print(json.dumps({"passed":passed,"record":str(folder / "result.json")}, indent=2))
    finally:
        docker("rm", "--force", name, check=False)
        docker("volume", "rm", volume, check=False)
        secret.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
