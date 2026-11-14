import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.channels.Channels;
import java.nio.channels.SocketChannel;
import java.nio.file.InvalidPathException;
import java.nio.file.Path;
import java.util.Locale;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.lang.instrument.Instrumentation;

public final class WindowBridge {
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
                System.getProperty("jvm.combiner.window.socket");

        if (configuredAddress == null || configuredAddress.isBlank()) {
            configuredAddress = System.getenv("JVM_COMBINER_WINDOW_SOCKET");
        }

        if (configuredAddress == null || configuredAddress.isBlank()) {
            System.err.println(
                    "[WindowBridge] Manager socket path is not configured");
            return;
        }

        final Path socketPath;
        try {
            socketPath = parseSocketPath(configuredAddress.trim());
        } catch (IllegalArgumentException error) {
            System.err.println(
                    "[WindowBridge] Invalid server socket path: " + error.getMessage());
            return;
        }

        Thread worker = new Thread(
                () -> runRegistrationLoop(loader, socketPath),
                "jvm-combiner-window-bridge");

        worker.setDaemon(true);
        worker.start();
    }

    private static Path parseSocketPath(String value) {
        final Path path;
        try {
            path = Path.of(value);
        } catch (InvalidPathException error) {
            throw new IllegalArgumentException(value, error);
        }

        if (!path.isAbsolute()) {
            throw new IllegalArgumentException("path must be absolute: " + value);
        }

        return path;
    }

    private static void runRegistrationLoop(String loader, Path socketPath) {
        long retrySeconds = INITIAL_RETRY_SECONDS;
        long pid = ProcessHandle.current().pid();

        while (!Thread.currentThread().isInterrupted()) {
            try {
                connectAndMaintain(loader, pid, socketPath);
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
            String loader, long pid, Path socketPath)
            throws IOException, RegistrationRejectedException {
        try (SocketChannel channel = SocketChannel.open(StandardProtocolFamily.UNIX)) {
            channel.connect(UnixDomainSocketAddress.of(socketPath));

            BufferedReader reader = new BufferedReader(
                    new InputStreamReader(
                            Channels.newInputStream(channel), StandardCharsets.UTF_8));

            BufferedWriter writer = new BufferedWriter(
                    new OutputStreamWriter(
                            Channels.newOutputStream(channel), StandardCharsets.UTF_8));

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
                            + " socket="
                            + socketPath);

            while (!Thread.currentThread().isInterrupted()) {
                if (reader.read() == -1) {
                    throw new IOException("server disconnected");
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
