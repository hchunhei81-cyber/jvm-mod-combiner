import java.math.BigDecimal;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Dependency-free strict JSON parser for the JVM Mod Combiner DataBus.
 *
 * Protocol limits are deliberately independent of Minecraft/Gson classloaders.
 */
final class StrictJson {
    static final int MAX_MESSAGE_BYTES = 64 * 1024;
    static final int MAX_NUMBER_CHARS = 128;
    private static final int MAX_DEPTH = 16;
    private static final int MAX_NODES = 256;
    private static final int MAX_OBJECT_FIELDS = 32;
    private static final int MAX_ARRAY_ITEMS = 64;
    private static final int MAX_STRING_CHARS = 16 * 1024;

    private StrictJson() {}

    static Map<String, Object> parseUtf8(byte[] bytes) {
        if (bytes == null || bytes.length == 0 ||
                bytes.length > MAX_MESSAGE_BYTES) {
            throw new IllegalArgumentException("Invalid JSON message size");
        }

        final String input;
        try {
            input = StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(bytes))
                    .toString();
        } catch (CharacterCodingException error) {
            throw new IllegalArgumentException("Invalid UTF-8", error);
        }

        return parse(input);
    }

    static Map<String, Object> parse(String input) {
        if (input == null || input.isEmpty()) {
            throw new IllegalArgumentException("Empty JSON input");
        }

        // Also protects callers that bypass parseUtf8().
        if (input.getBytes(StandardCharsets.UTF_8).length > MAX_MESSAGE_BYTES) {
            throw new IllegalArgumentException("JSON message too large");
        }

        Parser parser = new Parser(input);
        Object value = parser.parseValue(0);
        parser.skipWhitespace();

        if (!parser.atEnd()) {
            throw parser.error("Trailing content");
        }
        if (!(value instanceof Map<?, ?>)) {
            throw parser.error("Top-level JSON value must be an object");
        }

        @SuppressWarnings("unchecked")
        Map<String, Object> object = (Map<String, Object>) value;
        return Collections.unmodifiableMap(object);
    }

    static void requireKeys(
            Map<String, Object> object,
            Set<String> required,
            Set<String> allowed) {
        if (object == null || required == null || allowed == null) {
            throw new IllegalArgumentException("Invalid schema arguments");
        }

        for (String key : object.keySet()) {
            if (!allowed.contains(key)) {
                throw new IllegalArgumentException("Unknown field: " + key);
            }
        }

        for (String key : required) {
            if (!object.containsKey(key)) {
                throw new IllegalArgumentException("Missing field: " + key);
            }
        }
    }

    static String string(Map<String, Object> object, String key) {
        Object value = object.get(key);
        if (!(value instanceof String)) {
            throw new IllegalArgumentException("Field must be a string: " + key);
        }
        return (String) value;
    }

    static boolean bool(Map<String, Object> object, String key) {
        Object value = object.get(key);
        if (!(value instanceof Boolean)) {
            throw new IllegalArgumentException("Field must be boolean: " + key);
        }
        return (Boolean) value;
    }

    static double finiteDouble(Map<String, Object> object, String key) {
        Object value = object.get(key);
        if (!(value instanceof BigDecimal)) {
            throw new IllegalArgumentException("Field must be a number: " + key);
        }

        double result = ((BigDecimal) value).doubleValue();
        if (!Double.isFinite(result)) {
            throw new IllegalArgumentException("Non-finite number: " + key);
        }
        return result;
    }

    static float finiteFloat(Map<String, Object> object, String key) {
        double value = finiteDouble(object, key);
        float result = (float) value;
        if (!Float.isFinite(result)) {
            throw new IllegalArgumentException("Float out of range: " + key);
        }
        return result;
    }

    static long positiveLong(Map<String, Object> object, String key) {
        Object value = object.get(key);
        if (!(value instanceof BigDecimal)) {
            throw new IllegalArgumentException("Field must be an integer: " + key);
        }

        try {
            long result = ((BigDecimal) value).longValueExact();
            if (result <= 0L) {
                throw new IllegalArgumentException("Integer must be positive: " + key);
            }
            return result;
        } catch (ArithmeticException error) {
            throw new IllegalArgumentException("Invalid integer: " + key, error);
        }
    }

    private static final class Parser {
        private final String input;
        private int position;
        private int nodes;

        Parser(String input) {
            this.input = input;
        }

        boolean atEnd() {
            return position == input.length();
        }

        IllegalArgumentException error(String message) {
            return new IllegalArgumentException(
                    message + " at JSON offset " + position);
        }

        void skipWhitespace() {
            while (!atEnd()) {
                char c = input.charAt(position);
                if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
                    position++;
                } else {
                    break;
                }
            }
        }

        Object parseValue(int depth) {
            skipWhitespace();
            countNode();

            if (depth > MAX_DEPTH) {
                throw error("Maximum nesting depth exceeded");
            }
            if (atEnd()) {
                throw error("Unexpected end of input");
            }

            char c = input.charAt(position);
            if (c == '{') return parseObject(depth + 1);
            if (c == '[') return parseArray(depth + 1);
            if (c == '"') return parseString();
            if (c == 't') {
                consumeLiteral("true");
                return Boolean.TRUE;
            }
            if (c == 'f') {
                consumeLiteral("false");
                return Boolean.FALSE;
            }
            if (c == 'n') {
                consumeLiteral("null");
                return null;
            }
            if (c == '-' || isDigit(c)) return parseNumber();

            throw error("Invalid JSON value");
        }

        private void countNode() {
            nodes++;
            if (nodes > MAX_NODES) {
                throw error("Maximum JSON node count exceeded");
            }
        }

        private Map<String, Object> parseObject(int depth) {
            position++; // {
            skipWhitespace();

            Map<String, Object> result = new LinkedHashMap<>();
            if (consume('}')) return result;

            while (true) {
                skipWhitespace();
                if (atEnd() || input.charAt(position) != '"') {
                    throw error("Object key must be a string");
                }

                String key = parseString();
                skipWhitespace();
                expect(':');

                Object value = parseValue(depth);

                if (result.containsKey(key)) {
                    throw error("Duplicate object key: " + key);
                }
                result.put(key, value);

                if (result.size() > MAX_OBJECT_FIELDS) {
                    throw error("Maximum object field count exceeded");
                }

                skipWhitespace();
                if (consume('}')) return result;
                expect(',');
            }
        }

        private List<Object> parseArray(int depth) {
            position++; // [
            skipWhitespace();

            List<Object> result = new ArrayList<>();
            if (consume(']')) return result;

            while (true) {
                result.add(parseValue(depth));
                if (result.size() > MAX_ARRAY_ITEMS) {
                    throw error("Maximum array item count exceeded");
                }

                skipWhitespace();
                if (consume(']')) return result;
                expect(',');
            }
        }

        private String parseString() {
            expect('"');
            StringBuilder result = new StringBuilder();

            while (!atEnd()) {
                char c = input.charAt(position++);

                if (c == '"') {
                    if (result.length() > MAX_STRING_CHARS) {
                        throw error("String too long");
                    }
                    return result.toString();
                }

                if (c < 0x20) {
                    throw error("Unescaped control character");
                }

                if (c == '\\') {
                    if (atEnd()) throw error("Incomplete escape");
                    char escaped = input.charAt(position++);

                    switch (escaped) {
                        case '"': result.append('"'); break;
                        case '\\': result.append('\\'); break;
                        case '/': result.append('/'); break;
                        case 'b': result.append('\b'); break;
                        case 'f': result.append('\f'); break;
                        case 'n': result.append('\n'); break;
                        case 'r': result.append('\r'); break;
                        case 't': result.append('\t'); break;
                        case 'u':
                            appendUnicodeEscape(result);
                            break;
                        default:
                            throw error("Invalid string escape");
                    }
                } else if (Character.isHighSurrogate(c)) {
                    if (atEnd() || !Character.isLowSurrogate(
                            input.charAt(position))) {
                        throw error("Unpaired high surrogate");
                    }
                    result.append(c);
                    result.append(input.charAt(position++));
                } else if (Character.isLowSurrogate(c)) {
                    throw error("Unpaired low surrogate");
                } else {
                    result.append(c);
                }

                if (result.length() > MAX_STRING_CHARS) {
                    throw error("String too long");
                }
            }

            throw error("Unterminated string");
        }

        private void appendUnicodeEscape(StringBuilder result) {
            char first = readHexChar();

            if (Character.isHighSurrogate(first)) {
                if (position + 2 > input.length() ||
                        input.charAt(position) != '\\' ||
                        input.charAt(position + 1) != 'u') {
                    throw error("High surrogate without low surrogate");
                }
                position += 2;

                char second = readHexChar();
                if (!Character.isLowSurrogate(second)) {
                    throw error("Invalid low surrogate");
                }

                result.append(first).append(second);
                return;
            }

            if (Character.isLowSurrogate(first)) {
                throw error("Unpaired low surrogate escape");
            }

            result.append(first);
        }

        private char readHexChar() {
            if (position + 4 > input.length()) {
                throw error("Incomplete Unicode escape");
            }

            int value = 0;
            for (int i = 0; i < 4; i++) {
                char c = input.charAt(position++);
                int digit;
                if (c >= '0' && c <= '9') {
                    digit = c - '0';
                } else if (c >= 'a' && c <= 'f') {
                    digit = c - 'a' + 10;
                } else if (c >= 'A' && c <= 'F') {
                    digit = c - 'A' + 10;
                } else {
                    throw error("Invalid Unicode escape");
                }
                value = (value << 4) | digit;
            }
            return (char) value;
        }

        private BigDecimal parseNumber() {
            int start = position;

            consume('-');

            if (consume('0')) {
                if (!atEnd() && isDigit(input.charAt(position))) {
                    throw error("Leading zero in number");
                }
            } else {
                requireDigitOneToNine();
                while (!atEnd() && isDigit(input.charAt(position))) {
                    position++;
                }
            }

            if (consume('.')) {
                if (atEnd() || !isDigit(input.charAt(position))) {
                    throw error("Missing fractional digits");
                }
                while (!atEnd() && isDigit(input.charAt(position))) {
                    position++;
                }
            }

            if (consume('e') || consume('E')) {
                if (!consume('+')) consume('-');

                if (atEnd() || !isDigit(input.charAt(position))) {
                    throw error("Missing exponent digits");
                }
                while (!atEnd() && isDigit(input.charAt(position))) {
                    position++;
                }
            }

            if (position - start > MAX_NUMBER_CHARS) {
                throw error("JSON number exceeds maximum token length");
            }

            try {
                return new BigDecimal(input.substring(start, position));
            } catch (NumberFormatException error) {
                throw error("Invalid JSON number");
            }
        }

        private void requireDigitOneToNine() {
            if (atEnd()) throw error("Expected digit");
            char c = input.charAt(position);
            if (c < '1' || c > '9') throw error("Invalid number");
            position++;
        }

        private void consumeLiteral(String literal) {
            if (!input.regionMatches(position, literal, 0, literal.length())) {
                throw error("Invalid literal");
            }
            position += literal.length();
        }

        private boolean consume(char expected) {
            if (!atEnd() && input.charAt(position) == expected) {
                position++;
                return true;
            }
            return false;
        }

        private void expect(char expected) {
            if (!consume(expected)) {
                throw error("Expected '" + expected + "'");
            }
        }

        private static boolean isDigit(char c) {
            return c >= '0' && c <= '9';
        }
    }
}
