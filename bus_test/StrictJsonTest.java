import java.util.Map;

public final class StrictJsonTest {
    private static int passed;

    private StrictJsonTest() {}

    public static void main(String[] args) {
        testValidObject();
        testDuplicateKeyRejected();
        testOversizedNumberRejected();
        testMaximumNumberAccepted();
        System.out.println("[PASS] StrictJson tests: " + passed + "/4");
    }

    private static void testValidObject() {
        Map<String, Object> parsed = StrictJson.parse("{\"value\":123}");
        if (StrictJson.finiteDouble(parsed, "value") != 123.0) {
            throw new AssertionError("Unexpected parsed numeric value");
        }
        pass("valid object");
    }

    private static void testDuplicateKeyRejected() {
        expectRejected(
                () -> StrictJson.parse("{\"x\":1,\"x\":2}"),
                "duplicate key");
    }

    private static void testOversizedNumberRejected() {
        String number = "1".repeat(StrictJson.MAX_NUMBER_CHARS + 1);
        expectRejected(
                () -> StrictJson.parse("{\"n\":" + number + "}"),
                "oversized number");
    }

    private static void testMaximumNumberAccepted() {
        String number = "1".repeat(StrictJson.MAX_NUMBER_CHARS);
        StrictJson.parse("{\"n\":" + number + "}");
        pass("maximum-length number");
    }

    private static void expectRejected(Runnable action, String name) {
        try {
            action.run();
        } catch (IllegalArgumentException expected) {
            pass(name);
            return;
        }
        throw new AssertionError("Expected rejection: " + name);
    }

    private static void pass(String name) {
        passed++;
        System.out.println("[PASS] " + name);
    }
}
