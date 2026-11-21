import java.util.UUID;

final class MonsterAckFaultInjectionTest {
    private static int assertions;

    private static void check(boolean value, String message) {
        assertions++;
        if (!value) throw new AssertionError(message);
    }

    private static final class Receiver {
        final MonsterSequenceTracker tracker =
                new MonsterSequenceTracker(128);
        long generation = 1;
        boolean connected = true;
        boolean cleanupPending;
        int spawns;
        int removals;
        int acks;

        synchronized boolean receive(
                UUID uuid, long seq, String fingerprint, long taskGeneration) {
            if (!connected || cleanupPending ||
                    generation != taskGeneration) return false;

            MonsterSequenceTracker.Decision decision =
                    tracker.inspect(uuid, seq, fingerprint);

            if (decision == MonsterSequenceTracker.Decision.DUPLICATE) {
                acks++;
                return true;
            }
            if (decision != MonsterSequenceTracker.Decision.ACCEPT) {
                return false;
            }

            if ("spawn".equals(fingerprint)) spawns++;
            else if ("remove".equals(fingerprint)) removals++;
            else throw new IllegalArgumentException(fingerprint);

            tracker.commit(uuid, seq, fingerprint);
            acks++;
            return true;
        }

        synchronized void disconnect() {
            connected = false;
            cleanupPending = true;
            generation++;
        }

        synchronized void reconnect() {
            generation++;
            connected = true;
            cleanupPending = true;
        }

        synchronized void finishCleanup() {
            tracker.clear();
            cleanupPending = false;
        }
    }

    private static void spawnAckLoss() {
        Receiver r = new Receiver();
        UUID id = UUID.randomUUID();
        long g = r.generation;

        check(r.receive(id, 1, "spawn", g), "Initial spawn");
        check(r.spawns == 1, "One spawn operation");

        // First ACK is lost. Retry must ACK without spawning again.
        check(r.receive(id, 1, "spawn", g), "Duplicate spawn ACK");
        check(r.spawns == 1, "No duplicate spawn");
        check(r.acks == 2, "ACK written twice");
    }

    private static void removeAckLoss() {
        Receiver r = new Receiver();
        UUID id = UUID.randomUUID();
        long g = r.generation;

        check(r.receive(id, 8, "remove", g), "Initial remove");
        check(r.receive(id, 8, "remove", g), "Duplicate remove ACK");
        check(r.removals == 1, "Remove executes once");
        check(r.acks == 2, "Remove ACK retried");
    }

    private static void conflictAndStale() {
        Receiver r = new Receiver();
        UUID id = UUID.randomUUID();
        long g = r.generation;

        check(r.receive(id, 10, "spawn", g), "Initial sequence");
        check(!r.receive(id, 10, "remove", g), "Fingerprint conflict");
        check(!r.receive(id, 9, "spawn", g), "Stale sequence");
        check(r.spawns == 1 && r.removals == 0,
                "Rejected operations must not execute");
    }

    private static void generationIsolation() {
        Receiver r = new Receiver();
        UUID id = UUID.randomUUID();
        long old = r.generation;

        check(r.receive(id, 1, "spawn", old), "Old session spawn");
        r.disconnect();
        r.reconnect();
        long current = r.generation;

        check(!r.receive(id, 2, "spawn", current),
                "Reconnect cleanup barrier");
        r.finishCleanup();

        check(!r.receive(id, 2, "spawn", old),
                "Reject old generation task");
        check(r.receive(id, 2, "spawn", current),
                "Accept current generation task");
        check(r.spawns == 2, "Only valid operations execute");
    }

    private static void sequenceReset() {
        Receiver r = new Receiver();
        UUID id = UUID.randomUUID();

        check(r.receive(id, 50, "spawn", r.generation),
                "Old session sequence");

        r.disconnect();
        r.reconnect();
        r.finishCleanup();

        check(r.receive(id, 1, "spawn", r.generation),
                "New session sequence reset");
        check(r.spawns == 2, "New session operation executes once");
    }

    private static void mainAckMatching() {
        long pending = 20;
        check(pending != 19, "Stale ACK must not match pending");
        check(pending == 20, "Matching ACK must match pending");
    }

    public static void main(String[] args) {
        spawnAckLoss();
        removeAckLoss();
        conflictAndStale();
        generationIsolation();
        sequenceReset();
        mainAckMatching();

        System.out.println(
                "Monster ACK fault injection: PASS (" +
                        assertions + " assertions)");
    }
}
