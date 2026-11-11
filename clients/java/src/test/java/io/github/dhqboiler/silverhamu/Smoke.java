package io.github.dhqboiler.silverhamu;

import java.nio.file.Path;
import java.util.Arrays;
import java.util.HexFormat;

public final class Smoke {
    private static void check(boolean condition, String detail) {
        if (!condition) throw new AssertionError(detail);
    }

    public static void main(String[] args) throws Exception {
        String host = System.getenv("CATDB_HOST");
        int port = Integer.parseInt(System.getenv("CATDB_PORT"));
        String token = System.getenv("CATDB_TOKEN");
        String ca = System.getenv("CATDB_CA_FILE");
        try (SilverHamuClient db = token != null ? SilverHamuClient.connectWithToken(host, port, token)
                : SilverHamuClient.connectWithAccount(host, port, System.getenv("CATDB_USER"),
                        System.getenv("CATDB_PASSWORD"), "main", ca == null ? null : Path.of(ca))) {
            db.ping();
            if ("1".equals(System.getenv("CATDB_AUTH_ONLY"))) return;
            HexFormat hex = HexFormat.of();
            byte[] shared = hex.parseHex("ff00");
            byte[] expected = hex.parseHex(System.getenv("CATDB_EXPECT"));
            byte[] next = hex.parseHex(System.getenv("CATDB_VALUE"));
            byte[] testKey = hex.parseHex("6a61766100fe");
            check(Arrays.equals(db.getLatest(shared), expected), "incoming value");
            db.deleteCommit(testKey);
            check(db.getLatest(testKey) == null, "missing value");
            db.putCommit(testKey, new byte[0]);
            check(Arrays.equals(db.getLatest(testKey), new byte[0]), "empty value");
            db.begin();
            db.delete(testKey);
            check(db.get(testKey) == null, "staged delete");
            db.rollback();
            db.beginRead();
            check(Arrays.equals(db.get(testKey), new byte[0]), "rolled-back delete");
            db.rollback();
            db.begin();
            db.put(testKey, next);
            db.commit();
            check(Arrays.equals(db.getLatest(testKey), next), "committed value");
            try {
                db.get(testKey);
                throw new AssertionError("expected no-transaction rejection");
            } catch (SilverHamuClient.ServerException e) {
                check(e.code().equals("invalid_request"), "wrong rejection");
            }
            db.deleteCommit(testKey);
            db.putCommit(shared, next);
        }
    }
}
