import java.util.UUID;

public final class MonsterSequenceTrackerTest {
    private static int passed;

    private static void check(
            String name,
            MonsterSequenceTracker.Decision actual,
            MonsterSequenceTracker.Decision expected) {
        if (actual != expected) {
            throw new AssertionError(
                    name + ": expected " + expected + ", got " + actual);
        }
        passed++;
        System.out.println("[PASS] " + name);
    }

    private static void check(
            String name, boolean actual, boolean expected) {
        if (actual != expected) {
            throw new AssertionError(name + ": expected " + expected);
        }
        passed++;
        System.out.println("[PASS] " + name);
    }

    public static void main(String[] args) {
        UUID a = UUID.fromString("00000000-0000-0000-0000-000000000001");
        UUID b = UUID.fromString("00000000-0000-0000-0000-000000000002");
        MonsterSequenceTracker tracker = new MonsterSequenceTracker(2);

        check("first operation accepted",
                tracker.inspect(a, 10, "spawn|zombie|1,2,3"),
                MonsterSequenceTracker.Decision.ACCEPT);
        tracker.commit(a, 10, "spawn|zombie|1,2,3");

        check("same sequence and payload is duplicate",
                tracker.inspect(a, 10, "spawn|zombie|1,2,3"),
                MonsterSequenceTracker.Decision.DUPLICATE);
        check("same sequence with changed payload is conflict",
                tracker.inspect(a, 10, "spawn|zombie|9,9,9"),
                MonsterSequenceTracker.Decision.CONFLICT);
        check("older sequence is stale",
                tracker.inspect(a, 9, "spawn|zombie|1,2,3"),
                MonsterSequenceTracker.Decision.STALE);

        check("higher sequence accepted",
                tracker.inspect(a, 12, "update|zombie|4,5,6"),
                MonsterSequenceTracker.Decision.ACCEPT);
        tracker.commit(a, 12, "update|zombie|4,5,6");

        // A remove fingerprint remains as a tombstone for this UUID.
        check("remove accepted",
                tracker.inspect(a, 13, "remove"),
                MonsterSequenceTracker.Decision.ACCEPT);
        tracker.commit(a, 13, "remove");
        check("remove retry is idempotent",
                tracker.inspect(a, 13, "remove"),
                MonsterSequenceTracker.Decision.DUPLICATE);
        check("old spawn cannot resurrect removed UUID",
                tracker.inspect(a, 12, "update|zombie|4,5,6"),
                MonsterSequenceTracker.Decision.STALE);

        tracker.commit(b, 1, "spawn|skeleton|0,0,0");
        check("capacity fails closed for new UUID",
                tracker.inspect(UUID.randomUUID(), 1, "spawn"),
                MonsterSequenceTracker.Decision.CAPACITY);
        check("existing UUID can still advance at capacity",
                tracker.inspect(a, 14, "update|zombie|7,8,9"),
                MonsterSequenceTracker.Decision.ACCEPT);

        tracker.clear();
        check("session cleanup clears sequence records", tracker.size() == 0, true);
        check("new session accepts reset sequence",
                tracker.inspect(a, 1, "spawn|zombie|1,2,3"),
                MonsterSequenceTracker.Decision.ACCEPT);

        System.out.println("[PASS] MonsterSequenceTracker tests: " + passed);
    }
}
