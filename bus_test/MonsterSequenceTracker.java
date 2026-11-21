import java.util.HashMap;
import java.util.Map;
import java.util.Objects;
import java.util.UUID;

/**
 * Per-UUID inbound sequence tracking.
 * Call inspect/commit on the NeoForge main thread.
 * Entries are deliberately never evicted within a session.
 */
final class MonsterSequenceTracker {
    enum Decision {
        ACCEPT, DUPLICATE, STALE, CONFLICT, CAPACITY
    }

    private record Entry(long sequence, String fingerprint) {}

    private final int maximumEntries;
    private final Map<UUID, Entry> entries = new HashMap<>();

    MonsterSequenceTracker(int maximumEntries) {
        if (maximumEntries < 1) {
            throw new IllegalArgumentException("maximumEntries must be positive");
        }
        this.maximumEntries = maximumEntries;
    }

    Decision inspect(UUID uuid, long sequence, String fingerprint) {
        Objects.requireNonNull(uuid, "uuid");
        Objects.requireNonNull(fingerprint, "fingerprint");
        if (sequence <= 0L) {
            throw new IllegalArgumentException("sequence must be positive");
        }

        Entry previous = entries.get(uuid);
        if (previous == null) {
            return entries.size() >= maximumEntries
                    ? Decision.CAPACITY : Decision.ACCEPT;
        }
        if (sequence < previous.sequence()) {
            return Decision.STALE;
        }
        if (sequence == previous.sequence()) {
            return fingerprint.equals(previous.fingerprint())
                    ? Decision.DUPLICATE : Decision.CONFLICT;
        }
        return Decision.ACCEPT;
    }

    void commit(UUID uuid, long sequence, String fingerprint) {
        Decision decision = inspect(uuid, sequence, fingerprint);
        if (decision != Decision.ACCEPT) {
            throw new IllegalStateException("Cannot commit sequence: " + decision);
        }
        entries.put(uuid, new Entry(sequence, fingerprint));
    }

    void clear() {
        entries.clear();
    }

    int size() {
        return entries.size();
    }
}
