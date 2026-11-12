import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.lang.instrument.Instrumentation;

public final class WindowBridge {
    private static final String DEFAULT_ADDRESS = "127.0.0.1:25566";
    private static final long INITIAL_RETRY_SECONDS = 1;
    private static final long MAX_RETRY_SECONDS = 30;

    private static final Set<String> SUPPORTED_LOADERS =
            Set.of("fabric", "forge", "neoforge");

    private static final Pattern REGISTERED =
            Pattern.compile("\"type\"\\s*:\\s*\"registered\"");
    private static final Pattern ACCEPTED =
            Pattern.compile("\"accepted\"\\s*:\\s*true");
    private static final Pattern ERROR =
            Pattern.compile("\"error\"\\s*:\\s*\"([^\"]*)\"");

    private WindowBridge() {}

    public static void premain(String agentArgs, Instrumentation instrumentation) {
        String loader = agentArgs == null
                ? ""
                : agentArgs.trim().toLowerCase(Locale.ROOT);

        if (!SUPPORTED_LOADERS.contains(loader)) {
            System.err.println(
                    "[WindowBridge] Invalid or missing loader argument: " + loader);
            return;
        }

        // JVM property takes precedence; environment variable is the fallback.
        String configuredAddress =
                System.getProperty("jvm.combiner.window.address");

        if (configuredAddress == null || configuredAddress.isBlank()) {
            configuredAddress = System.getenv("JVM_COMBINER_WINDOW_ADDR");
        }

        if (configuredAddress == null || configuredAddress.isBlank()) {
            configuredAddress = DEFAULT_ADDRESS;
        }

        final InetSocketAddress address;
        try {
            address = parseAddress(configuredAddress.trim());
        } catch (IllegalArgumentException error) {
            System.err.println(
                    "[WindowBridge] Invalid server address: " + error.getMessage());
            return;
        }

        Thread worker = new Thread(
                () -> runRegistrationLoop(loader, address),
                "jvm-combiner-window-bridge");

        worker.setDaemon(true);
        worker.start();
    }

    private static InetSocketAddress parseAddress(String value) {
        int separator = value.lastIndexOf(':');
        if (separator <= 0 || separator == value.length() - 1) {
            throw new IllegalArgumentException(value);
        }

        String host = value.substring(0, separator);
        if (host.startsWith("[") && host.endsWith("]")) {
            host = host.substring(1, host.length() - 1);
        }

        final int port;
        try {
            port = Integer.parseInt(value.substring(separator + 1));
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException(value, error);
        }

        if (port < 1 || port > 65535 || host.isBlank()) {
            throw new IllegalArgumentException(value);
        }

        return new InetSocketAddress(host, port);
    }

    private static void runRegistrationLoop(
            String loader, InetSocketAddress address) {
        long retrySeconds = INITIAL_RETRY_SECONDS;
        long pid = ProcessHandle.current().pid();

        while (!Thread.currentThread().isInterrupted()) {
            try {
                connectAndMaintain(loader, pid, address);
                retrySeconds = INITIAL_RETRY_SECONDS;
            } catch (RegistrationRejectedException error) {
                System.err.println(
                        "[WindowBridge] Registration rejected; stopping: "
                                + error.getMessage());
                return;
            } catch (IOException error) {
                System.err.println(
                        "[WindowBridge] Connection failed: "
                                + error.getMessage()
                                + "; retrying in "
                                + retrySeconds
                                + "s");
            }

            try {
                Thread.sleep(retrySeconds * 1000);
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                return;
            }

            retrySeconds = Math.min(retrySeconds * 2, MAX_RETRY_SECONDS);
        }
    }

    private static void connectAndMaintain(
            String loader, long pid, InetSocketAddress address)
            throws IOException, RegistrationRejectedException {
        try (Socket socket = new Socket()) {
            socket.setKeepAlive(true);
            socket.connect(address, 3000);
            socket.setSoTimeout(5000);

            BufferedReader reader = new BufferedReader(
                    new InputStreamReader(
                            socket.getInputStream(), StandardCharsets.UTF_8));

            BufferedWriter writer = new BufferedWriter(
                    new OutputStreamWriter(
                            socket.getOutputStream(), StandardCharsets.UTF_8));

            String request = String.format(
                    Locale.ROOT,
                    "{\"type\":\"register\",\"pid\":%d,"
                            + "\"loader\":\"%s\","
                            + "\"role\":\"minecraft_client\","
                            + "\"protocol\":1}",
                    pid,
                    loader);

            writer.write(request);
            writer.newLine();
            writer.flush();

            String response = reader.readLine();
            if (response == null) {
                throw new IOException("server closed before registration response");
            }

            if (!REGISTERED.matcher(response).find()) {
                throw new IOException("invalid registration response");
            }

            if (!ACCEPTED.matcher(response).find()) {
                Matcher error = ERROR.matcher(response);
                String reason = error.find() ? error.group(1) : "unspecified error";
                throw new RegistrationRejectedException(reason);
            }

            System.out.println(
                    "[WindowBridge] Registered loader="
                            + loader
                            + " pid="
                            + pid
                            + " server="
                            + address);

            while (!Thread.currentThread().isInterrupted()) {
                try {
                    if (reader.read() == -1) {
                        throw new IOException("server disconnected");
                    }
                } catch (SocketTimeoutException ignored) {
                    // The server keeps the registration socket open.
                }
            }
        }
    }

    private static final class RegistrationRejectedException extends Exception {
        private RegistrationRejectedException(String message) {
            super(message);
        }
    }
}
