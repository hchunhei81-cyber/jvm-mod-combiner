import java.io.BufferedWriter;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStreamWriter;
import java.net.StandardProtocolFamily;
import java.net.UnixDomainSocketAddress;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.nio.channels.Channels;
import java.nio.channels.SocketChannel;
import java.nio.file.InvalidPathException;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.lang.instrument.Instrumentation;

public final class WindowBridge {
    private static final long INITIAL_RETRY_SECONDS = 1;
    private static final long MAX_RETRY_SECONDS = 30;

    private static final Set<String> SUPPORTED_LOADERS =
            Set.of("fabric", "forge", "neoforge");

    private static final int MAX_RESPONSE_BYTES = 16 * 1024;

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

            InputStream input = Channels.newInputStream(channel);

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

            String response = readBoundedResponseLine(input);
            validateRegistrationResponse(response, pid);

            System.out.println(
                    "[WindowBridge] Registered loader="
                            + loader
                            + " pid="
                            + pid
                            + " socket="
                            + socketPath);

            while (!Thread.currentThread().isInterrupted()) {
                if (input.read() == -1) {
                    throw new IOException("server disconnected");
                }
            }
        }
    }

    private static String readBoundedResponseLine(InputStream input)
            throws IOException {
        ByteArrayOutputStream buffer = new ByteArrayOutputStream(256);

        while (true) {
            int value = input.read();

            if (value == -1) {
                throw new IOException(
                        "server closed before completing registration response");
            }

            if (value == '\n') {
                break;
            }

            if (buffer.size() >= MAX_RESPONSE_BYTES) {
                throw new IOException(
                        "registration response exceeds 16 KiB limit");
            }

            buffer.write(value);
        }

        byte[] bytes = buffer.toByteArray();

        // Accept CRLF as well as LF.
        int length = bytes.length;
        if (length > 0 && bytes[length - 1] == '\r') {
            length--;
        }

        try {
            return StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(bytes, 0, length))
                    .toString();
        } catch (CharacterCodingException error) {
            throw new IOException(
                    "registration response is not valid UTF-8",
                    error);
        }
    }

    private static void validateRegistrationResponse(
            String response, long expectedPid)
            throws IOException, RegistrationRejectedException {
        Map<String, Object> fields =
                new StrictJsonObjectParser(response).parse();

        Object type = fields.get("type");
        Object accepted = fields.get("accepted");

        if (!"registered".equals(type)) {
            throw new IOException(
                    "registration response has invalid type");
        }

        if (!(accepted instanceof Boolean)) {
            throw new IOException(
                    "registration response has invalid accepted field");
        }

        if (Boolean.TRUE.equals(accepted)) {
            if (fields.size() != 3
                    || !fields.containsKey("pid")
                    || !fields.keySet().containsAll(
                            Set.of("type", "accepted", "pid"))) {
                throw new IOException(
                        "invalid successful registration response schema");
            }

            Object pidValue = fields.get("pid");

            if (!(pidValue instanceof Long)) {
                throw new IOException(
                        "registration response PID is not an integer");
            }

            long responsePid = (Long) pidValue;

            if (responsePid <= 0
                    || responsePid > 0xffff_ffffL
                    || responsePid != expectedPid) {
                throw new IOException(
                        "registration response PID mismatch");
            }

            return;
        }

        if (fields.size() != 3
                || !fields.keySet().containsAll(
                        Set.of("type", "accepted", "error"))) {
            throw new IOException(
                    "invalid rejected registration response schema");
        }

        Object errorValue = fields.get("error");

        if (!(errorValue instanceof String)
                || ((String) errorValue).isBlank()) {
            throw new IOException(
                    "registration rejection has no valid error message");
        }

        throw new RegistrationRejectedException((String) errorValue);
    }

    private static final class RegistrationRejectedException extends Exception {
        private RegistrationRejectedException(String message) {
            super(message);
        }
    }

    private static final class StrictJsonObjectParser {

        private final String input;
        private int position;

        private StrictJsonObjectParser(String input) {
            this.input = input;
        }

        private Map<String, Object> parse() throws IOException {
            skipWhitespace();
            expect('{');

            Map<String, Object> result = new HashMap<>();

            skipWhitespace();

            if (consume('}')) {
                throw invalid();
            }

            while (true) {
                skipWhitespace();

                String key = parseString();

                if (result.containsKey(key)) {
                    throw new IOException(
                            "duplicate JSON response field: " + key);
                }

                skipWhitespace();
                expect(':');
                skipWhitespace();

                Object value = parseValue();
                result.put(key, value);

                skipWhitespace();

                if (consume('}')) {
                    break;
                }

                expect(',');
            }

            skipWhitespace();

            if (position != input.length()) {
                throw invalid();
            }

            return result;
        }

        private Object parseValue() throws IOException {
            if (position >= input.length()) {
                throw invalid();
            }

            char current = input.charAt(position);

            if (current == '"') {
                return parseString();
            }

            if (current == 't') {
                expectLiteral("true");
                return Boolean.TRUE;
            }

            if (current == 'f') {
                expectLiteral("false");
                return Boolean.FALSE;
            }

            if (current >= '0' && current <= '9') {
                return parsePositiveInteger();
            }

            throw invalid();
        }

        private String parseString() throws IOException {
            expect('"');

            StringBuilder result = new StringBuilder();

            while (position < input.length()) {
                char current = input.charAt(position++);

                if (current == '"') {
                    return result.toString();
                }

                if (current < 0x20) {
                    throw invalid();
                }

                if (current != '\\') {
                    result.append(current);
                    continue;
                }

                if (position >= input.length()) {
                    throw invalid();
                }

                char escaped = input.charAt(position++);

                switch (escaped) {
                    case '"':
                        result.append('"');
                        break;
                    case '\\':
                        result.append('\\');
                        break;
                    case '/':
                        result.append('/');
                        break;
                    case 'b':
                        result.append('\b');
                        break;
                    case 'f':
                        result.append('\f');
                        break;
                    case 'n':
                        result.append('\n');
                        break;
                    case 'r':
                        result.append('\r');
                        break;
                    case 't':
                        result.append('\t');
                        break;
                    case 'u':
                        result.append(parseUnicodeEscape());
                        break;
                    default:
                        throw invalid();
                }
            }

            throw invalid();
        }

        private char parseUnicodeEscape() throws IOException {
            if (position + 4 > input.length()) {
                throw invalid();
            }

            int value = 0;

            for (int i = 0; i < 4; i++) {
                char character = input.charAt(position++);
                int digit;

                if (character >= '0' && character <= '9') {
                    digit = character - '0';
                } else if (character >= 'a' && character <= 'f') {
                    digit = character - 'a' + 10;
                } else if (character >= 'A' && character <= 'F') {
                    digit = character - 'A' + 10;
                } else {
                    throw invalid();
                }

                value = (value << 4) | digit;
            }

            return (char) value;
        }

        private Long parsePositiveInteger() throws IOException {
            int start = position;

            if (input.charAt(position) == '0') {
                position++;

                if (position < input.length()) {
                    char next = input.charAt(position);

                    if (next >= '0' && next <= '9') {
                        throw invalid();
                    }
                }
            } else {
                while (position < input.length()) {
                    char current = input.charAt(position);

                    if (current < '0' || current > '9') {
                        break;
                    }

                    position++;
                }
            }

            try {
                return Long.parseLong(input.substring(start, position));
            } catch (NumberFormatException error) {
                throw invalid();
            }
        }

        private void expectLiteral(String literal) throws IOException {
            if (!input.startsWith(literal, position)) {
                throw invalid();
            }

            position += literal.length();
        }

        private void expect(char expected) throws IOException {
            if (!consume(expected)) {
                throw invalid();
            }
        }

        private boolean consume(char expected) {
            if (position < input.length()
                    && input.charAt(position) == expected) {
                position++;
                return true;
            }

            return false;
        }

        private void skipWhitespace() {
            while (position < input.length()) {
                char current = input.charAt(position);

                if (current == ' '
                        || current == '\t'
                        || current == '\r'
                        || current == '\n') {
                    position++;
                } else {
                    break;
                }
            }
        }

        private IOException invalid() {
            return new IOException(
                    "malformed registration response JSON at offset "
                            + position);
        }
    }
}
