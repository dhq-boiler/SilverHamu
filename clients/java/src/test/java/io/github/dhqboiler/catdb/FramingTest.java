package io.github.dhqboiler.catdb;

import static org.junit.Assert.*;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.Callable;
import java.util.concurrent.FutureTask;
import java.util.concurrent.TimeUnit;
import org.junit.Test;

public final class FramingTest {
    private static String read(DataInputStream input) throws IOException {
        int size = input.readInt();
        if (size < 1 || size > 1024 * 1024) throw new IOException("Invalid test request");
        byte[] bytes = new byte[size];
        input.readFully(bytes);
        return new String(bytes, StandardCharsets.UTF_8);
    }

    private static void send(DataOutputStream output, String body) throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        output.writeInt(bytes.length);
        for (byte b : bytes) output.writeByte(b);
        output.flush();
    }

    @Test public void invalidOrIncompleteResponsesCloseTheConnection() throws Exception {
        for (String fault : new String[]{"oversized", "truncated", "utf8", "json", "status", "hex"}) {
            try (ServerSocket listener = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
                Callable<Void> peer = () -> {
                    try (Socket socket = listener.accept()) {
                        socket.setSoTimeout(3_000);
                        DataInputStream in = new DataInputStream(socket.getInputStream());
                        DataOutputStream out = new DataOutputStream(socket.getOutputStream());
                        assertTrue(read(in).contains("\"op\":\"auth\""));
                        send(out, "{\"ok\":true,\"protocol\":1}");
                        assertTrue(read(in).contains("\"op\":\"get_latest\""));
                        switch (fault) {
                            case "oversized" -> out.writeInt(2 * 1024 * 1024 + 8193);
                            case "truncated" -> {
                                out.writeInt(128);
                                out.writeByte('{');
                                socket.shutdownOutput();
                            }
                            case "utf8" -> {
                                out.writeInt(1);
                                out.writeByte(0xff);
                            }
                            case "json" -> send(out, "{");
                            case "status" -> send(out, "{\"ok\":\"true\",\"value\":null}");
                            case "hex" -> send(out, "{\"ok\":true,\"value\":\"ff0\"}");
                            default -> throw new AssertionError(fault);
                        }
                        assertEquals(fault, -1, in.read());
                    }
                    return null;
                };
                FutureTask<Void> result = new FutureTask<>(peer);
                Thread server = new Thread(result);
                server.setDaemon(true);
                server.start();
                try (CatdbClient client = CatdbClient.connectWithToken(
                        listener.getInetAddress().getHostAddress(), listener.getLocalPort(), "token")) {
                    assertThrows(fault, IOException.class, () -> client.getLatest(new byte[]{0x01}));
                    assertThrows(fault, IOException.class, client::ping);
                    result.get(5, TimeUnit.SECONDS);
                }
            }
        }
    }
}
