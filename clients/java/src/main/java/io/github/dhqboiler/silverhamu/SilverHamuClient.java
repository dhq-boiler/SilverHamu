package io.github.dhqboiler.silverhamu;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParseException;
import com.google.gson.Strictness;
import java.io.BufferedOutputStream;
import java.io.Closeable;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.util.HexFormat;
import java.util.Objects;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;

/**
 * An authenticated TCP connection to a silverhamu service. Use try-with-resources to close it.
 * Each connection owns its transaction, so use a separate client for each concurrent transaction.
 *
 * <p>Keys and values are binary. Versions are unsigned 64-bit numbers represented as decimal
 * strings. A server rejection raises {@link ServerException}. If the response to a write is lost,
 * the write may have committed. Reconnect and inspect the application state before retrying.
 */
public final class SilverHamuClient implements Closeable {
    private static final int MAX_REQUEST = 1024 * 1024;
    private static final int MAX_RESPONSE = 2 * 1024 * 1024 + 8192;
    private static final int TIMEOUT_MS = 10_000;
    private static final Gson JSON = new GsonBuilder()
            .setStrictness(Strictness.STRICT).disableHtmlEscaping().serializeNulls().create();
    private static final HexFormat HEX = HexFormat.of();
    private final Socket socket;
    private final DataInputStream input;
    private final DataOutputStream output;

    private SilverHamuClient(Socket socket) throws IOException {
        this.socket = socket;
        input = new DataInputStream(socket.getInputStream());
        output = new DataOutputStream(new BufferedOutputStream(socket.getOutputStream()));
    }

    /**
     * Connects to a shared-token listener using plaintext TCP. Use loopback or an encrypted tunnel.
     * @param host server hostname or IP address
     * @param port service TCP port
     * @param token shared token configured on the server
     * @return an authenticated client
     * @throws IOException if the connection or authentication fails
     */
    public static SilverHamuClient connectWithToken(String host, int port, String token) throws IOException {
        JsonObject auth = operation("auth");
        auth.addProperty("version", 1);
        auth.addProperty("token", Objects.requireNonNull(token));
        return connect(host, port, auth, null);
    }

    /**
     * Connects to the selected database using account credentials over TLS 1.3.
     * The certificate chain and hostname are verified before credentials are sent.
     * @param host server name or IP address covered by the server certificate
     * @param port service TCP port
     * @param username silverhamu account name
     * @param password account password
     * @param database database name, such as {@code "main"}
     * @param caFile path to a trusted PEM CA bundle, or {@code null} to use the JVM trust store
     * @return an authenticated client bound to the selected database
     * @throws IOException if loading the trust material, connecting, or authenticating fails
     */
    public static SilverHamuClient connectWithAccount(String host, int port, String username, String password,
                                                 String database, Path caFile) throws IOException {
        JsonObject auth = operation("auth");
        auth.addProperty("version", 2);
        auth.addProperty("username", Objects.requireNonNull(username));
        auth.addProperty("password", Objects.requireNonNull(password));
        auth.addProperty("database", Objects.requireNonNull(database));
        return connect(host, port, auth, tlsContext(caFile));
    }

    private static SSLContext tlsContext(Path caFile) throws IOException {
        try {
            KeyStore trustStore = null;
            if (caFile != null) {
                trustStore = KeyStore.getInstance(KeyStore.getDefaultType());
                trustStore.load(null, null);
                try (InputStream pem = Files.newInputStream(caFile)) {
                    int count = 0;
                    for (Certificate cert : CertificateFactory.getInstance("X.509").generateCertificates(pem)) {
                        trustStore.setCertificateEntry("ca-" + count++, cert);
                    }
                    if (count == 0) throw new IOException("No CA certificates in " + caFile);
                }
            }
            TrustManagerFactory trust = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            trust.init(trustStore);
            SSLContext context = SSLContext.getInstance("TLSv1.3");
            context.init(null, trust.getTrustManagers(), null);
            return context;
        } catch (GeneralSecurityException e) {
            throw new IOException("Cannot initialize TLS trust", e);
        }
    }

    private static SilverHamuClient connect(String host, int port, JsonObject auth, SSLContext tls)
            throws IOException {
        Socket socket = new Socket();
        try {
            socket.connect(new InetSocketAddress(host, port), TIMEOUT_MS);
            socket.setSoTimeout(TIMEOUT_MS);
            socket.setTcpNoDelay(true);
            if (tls != null) {
                SSLSocket secure = (SSLSocket) tls.getSocketFactory().createSocket(socket, host, port, true);
                socket = secure;
                SSLParameters params = secure.getSSLParameters();
                params.setProtocols(new String[]{"TLSv1.3"});
                params.setEndpointIdentificationAlgorithm("HTTPS");
                secure.setSSLParameters(params);
                secure.startHandshake();
            }
            SilverHamuClient client = new SilverHamuClient(socket);
            JsonObject welcome = client.request(auth);
            if (!auth.get("version").equals(welcome.get("protocol"))) {
                throw new IOException("Unexpected silverhamu protocol version");
            }
            return client;
        } catch (IOException | RuntimeException e) {
            try { socket.close(); } catch (IOException closeError) { e.addSuppressed(closeError); }
            throw e;
        }
    }

    /**
     * Sends a wire operation, including operations such as paginated scans. Use hexadecimal
     * strings for binary request fields. Successive {@code scan} calls in the same transaction
     * read the same snapshot. Continue with {@code next_after}, even when a page has no rows.
     * @param message JSON object containing the operation name in {@code op} and its wire fields
     * @return the complete successful server response
     * @throws IllegalArgumentException if the encoded request exceeds 1 MiB
     * @throws ServerException if the service rejects the operation
     * @throws IOException if the exchange fails or the response is invalid
     */
    public synchronized JsonObject request(JsonObject message) throws IOException {
        byte[] body = JSON.toJson(Objects.requireNonNull(message)).getBytes(StandardCharsets.UTF_8);
        if (body.length > MAX_REQUEST) throw new IllegalArgumentException("Request exceeds 1 MiB");
        try {
            output.writeInt(body.length);
            output.write(body);
            output.flush();
            int size = input.readInt();
            if (size <= 0 || size > MAX_RESPONSE) throw new IOException("Invalid response size");
            byte[] data = new byte[size];
            input.readFully(data);
            String text = StandardCharsets.UTF_8.newDecoder().decode(ByteBuffer.wrap(data)).toString();
            JsonObject reply = JSON.fromJson(text, JsonObject.class);
            JsonElement ok = reply == null ? null : reply.get("ok");
            if (ok == null || !ok.isJsonPrimitive() || !ok.getAsJsonPrimitive().isBoolean()) {
                throw new IOException("Invalid response status");
            }
            if (!ok.getAsBoolean()) {
                String code = requireString(reply, "error");
                if (code.equals("unauthorized")) close();
                throw new ServerException(code);
            }
            return reply;
        } catch (ServerException e) {
            throw e;
        } catch (IOException | JsonParseException e) {
            try { close(); } catch (IOException closeError) { e.addSuppressed(closeError); }
            throw new IOException("silverhamu response unavailable; a submitted mutation may have committed", e);
        }
    }

    private static JsonObject operation(String op) {
        JsonObject message = new JsonObject();
        message.addProperty("op", op);
        return message;
    }

    private static JsonObject keyed(String op, byte[] key) {
        JsonObject message = operation(op);
        message.addProperty("key", HEX.formatHex(key));
        return message;
    }

    private static JsonObject write(String op, byte[] key, byte[] value) {
        JsonObject message = keyed(op, key);
        message.addProperty("value", HEX.formatHex(value));
        return message;
    }

    private static String requireString(JsonObject reply, String field) throws IOException {
        JsonElement value = reply.get(field);
        if (value == null || !value.isJsonPrimitive() || !value.getAsJsonPrimitive().isString()) {
            throw new IOException("Invalid silverhamu response field " + field);
        }
        return value.getAsString();
    }

    private synchronized String version(JsonObject message) throws IOException {
        JsonObject reply = request(message);
        try {
            return requireString(reply, "version");
        } catch (IOException e) {
            close();
            throw e;
        }
    }

    private synchronized byte[] readValue(JsonObject message) throws IOException {
        JsonObject reply = request(message);
        JsonElement value = reply.get("value");
        if (value != null && value.isJsonNull()) return null;
        try {
            return HEX.parseHex(requireString(reply, "value"));
        } catch (IOException | IllegalArgumentException e) {
            close();
            throw new IOException("Invalid silverhamu value", e);
        }
    }

    /**
     * Reads the current database version.
     * @return the current version as a decimal string
     * @throws IOException if the service rejects the request or the exchange fails
     */
    public String ping() throws IOException { return version(operation("ping")); }

    /**
     * Reads a key in a single request when no transaction is active on this connection.
     * @param key binary key to look up
     * @return the stored bytes, which may be empty, or {@code null} for a missing key
     * @throws IOException if the service rejects the request or the exchange fails
     */
    public byte[] getLatest(byte[] key) throws IOException { return readValue(keyed("get_latest", key)); }
    /**
     * Stores a value and waits for its durable commit. Requires no active transaction.
     * @param key binary key to write
     * @param value bytes to store, which may be empty
     * @return the committed version as a decimal string
     * @throws IOException if the service rejects the write or its acknowledgement is unavailable
     */
    public String putCommit(byte[] key, byte[] value) throws IOException { return version(write("put_commit", key, value)); }

    /**
     * Deletes a key and waits for its durable commit. Requires no active transaction.
     * @param key binary key to delete
     * @return the committed version as a decimal string
     * @throws IOException if the service rejects the deletion or its acknowledgement is unavailable
     */
    public String deleteCommit(byte[] key) throws IOException { return version(keyed("delete_commit", key)); }
    /**
     * Starts a read/write transaction. Its snapshot remains pinned until the transaction ends.
     * @return the snapshot version as a decimal string
     * @throws IOException if the service rejects the request or the exchange fails
     */
    public String begin() throws IOException { return version(operation("begin")); }

    /**
     * Starts a read-only transaction for a stable snapshot across multiple requests.
     * @return the snapshot version as a decimal string
     * @throws IOException if the service rejects the request or the exchange fails
     */
    public String beginRead() throws IOException { return version(operation("begin_read")); }
    /**
     * Reads a key in the active transaction, including changes already staged by that transaction.
     * @param key binary key to look up
     * @return the visible bytes, which may be empty, or {@code null} if the key is absent
     * @throws IOException if the service rejects the request or the exchange fails
     */
    public byte[] get(byte[] key) throws IOException { return readValue(keyed("get", key)); }

    /**
     * Stages a write in the active read/write transaction.
     * @param key binary key to write
     * @param value bytes to store, which may be empty
     * @throws IOException if the service rejects the write or the exchange fails
     */
    public void put(byte[] key, byte[] value) throws IOException { request(write("put", key, value)); }
    /**
     * Stages a deletion in the active read/write transaction.
     * @param key binary key to delete
     * @throws IOException if the service rejects the deletion or the exchange fails
     */
    public void delete(byte[] key) throws IOException { request(keyed("delete", key)); }

    /**
     * Finishes the active transaction and waits for its commit acknowledgement.
     * A conflicting write consumes the transaction and raises a {@link ServerException}.
     * @return the committed version as a decimal string
     * @throws IOException if the service rejects the commit or its acknowledgement is unavailable
     */
    public String commit() throws IOException { return version(operation("commit")); }
    /**
     * Ends the active transaction and discards any staged writes.
     * @throws IOException if the service rejects the rollback or the exchange fails
     */
    public void rollback() throws IOException { request(operation("rollback")); }

    /**
     * Closes the connection. The service discards its uncommitted transaction on disconnect.
     * @throws IOException if the underlying socket cannot be closed
     */
    @Override public void close() throws IOException { socket.close(); }

    /** A rejection returned by silverhamu, carrying the service's machine-readable error code. */
    public static final class ServerException extends IOException {
        /** Error code returned in the wire response. */
        private final String code;
        private ServerException(String code) {
            super("silverhamu rejected the request: " + code);
            this.code = code;
        }
        /**
         * Returns the code from the service response.
         * @return the wire error identifier, such as {@code conflict} or {@code unauthorized}
         */
        public String code() { return code; }
    }
}
