import os

from catdb_client import CatdbError, Client

env = os.environ
auth = ({"token": env["CATDB_TOKEN"]} if "CATDB_TOKEN" in env else
        {"username": env["CATDB_USER"], "password": env["CATDB_PASSWORD"], "cafile": env.get("CATDB_CA_FILE")})
with Client(env["CATDB_HOST"], int(env["CATDB_PORT"]), **auth) as db:
    assert db.request(op="ping")["ok"]
    if env.get("CATDB_AUTH_ONLY") != "1":
        key = bytes.fromhex("ff00")
        value = bytes.fromhex(env["CATDB_VALUE"])
        assert db.get_latest(key) == bytes.fromhex(env["CATDB_EXPECT"])
        probe = b"python\x00"
        db.delete_commit(probe)
        assert db.get_latest(probe) is None
        db.begin()
        db.put(probe, b"")
        assert db.get(probe) == b""
        db.rollback()
        assert db.get_latest(probe) is None
        db.begin()
        db.put(probe, value)
        db.commit()
        assert db.get_latest(probe) == value
        db.begin(read_only=True)
        assert db.get(probe) == value
        db.rollback()
        try:
            db.commit()
        except CatdbError as error:
            assert error.code == "invalid_request", error.code
        else:
            raise AssertionError("commit without a transaction succeeded")
        db.put_commit(key, value)
